// Ports expression-test/expression_test_runner.hpp and
// expression-test/expression_test_runner.cpp (minus UPDATE mode).

use std::fmt::Write;

use feature::{Value, ValueObject};
use mln_expression::Expression;
use mln_expression::expression::{evaluate_feature_with_canonical, evaluate_feature_with_images};

use crate::parser::{
    TestData, TestResult, compiled_to_value, expression_value_to_value, parse_expression, to_document, to_json,
};
use crate::test_runner_common::{color_output_equal, deep_equal, deep_equal_opt, tokenize};

#[derive(Clone, Debug, Default)]
pub struct TestRunOutput {
    pub id: String,
    pub passed: bool,
    pub text: String,
    pub expression: String,
    pub serialized: String,
    /// When the test fails only because some inputs evaluate differently,
    /// their indices in the fixture's `inputs`; empty otherwise.
    pub failed_inputs: Vec<usize>,
}

#[derive(Default)]
pub struct TestStats {
    pub passed: Vec<TestRunOutput>,
    pub failed: Vec<TestRunOutput>,
    pub errored: Vec<TestRunOutput>,
    pub ignore_passed: Vec<TestRunOutput>,
    pub ignore_failed: Vec<TestRunOutput>,
    pub ids: Vec<String>,
}

impl TestStats {
    #[must_use]
    pub fn test_count(&self) -> usize {
        self.passed.len() + self.failed.len() + self.errored.len() + self.ignore_passed.len() + self.ignore_failed.len()
    }
}

fn simple_diff(result: &Value, expected: &Value) -> String {
    let result_tokens = tokenize(&to_json(result));
    let expected_tokens = tokenize(&to_json(expected));
    let max_length = result_tokens.len().max(expected_tokens.len());
    let mut diff = String::new();
    let flush = |vec: &[String], pos: usize, out: &mut String, separator: &str| {
        for token in &vec[pos..] {
            out.push_str(separator);
            out.push_str(token);
            out.push('\n');
        }
    };

    for i in 0..max_length {
        if result_tokens.len() <= i {
            flush(&expected_tokens, i, &mut diff, "-");
            break;
        }

        if expected_tokens.len() <= i {
            flush(&result_tokens, i, &mut diff, "+");
            break;
        }

        if result_tokens[i] == expected_tokens[i] {
            let _ = writeln!(diff, "{}", result_tokens[i]);
        } else {
            let _ = writeln!(diff, "-{}\n+{}", expected_tokens[i], result_tokens[i]);
        }
    }
    diff
}

/// The indices of the inputs whose outputs differ from the expected ones, or
/// `None` when the outputs don't line up with them.
fn differing_inputs(outputs: Option<&Value>, expected: Option<&Value>, is_color: bool) -> Option<Vec<usize>> {
    let equal = if is_color { color_output_equal } else { deep_equal };
    match (outputs, expected) {
        (None, None) => Some(Vec::new()),
        (Some(Value::Array(outputs)), Some(Value::Array(expected))) if outputs.len() == expected.len() => {
            Some((0..outputs.len()).filter(|&i| !equal(&outputs[i], &expected[i])).collect())
        }
        _ => None,
    }
}

fn rewrite_roundtripped_type(expected: &str, actual: &mut String) {
    if expected.starts_with("array") && actual.starts_with("array") {
        *actual = expected.to_owned();
    }
}

fn evaluate_expression(data_inputs: &[crate::parser::Input], expression: &dyn Expression, result: &mut TestResult) {
    let mut outputs = Vec::new();
    for input in data_inputs {
        let evaluation_result = if let Some(canonical) = &input.canonical {
            evaluate_feature_with_canonical(
                expression,
                input.zoom,
                &input.feature,
                input.heatmap_density,
                &input.available_images,
                canonical,
            )
        } else {
            evaluate_feature_with_images(
                expression,
                input.zoom,
                &input.feature,
                input.heatmap_density,
                &input.available_images,
            )
        };
        match evaluation_result {
            Err(e) => {
                let mut error = ValueObject::new();
                error.insert("error".to_owned(), Value::String(e.message));
                outputs.push(Value::Object(error));
            }
            Ok(v) => {
                let value = expression_value_to_value(&v).expect("output value");
                outputs.push(value);
            }
        }
    }
    result.outputs = Some(Value::Array(outputs));
}

/// `runExpressionTest`.
pub fn run_expression_test(data: &mut TestData, id: &str) -> TestRunOutput {
    let mut output = TestRunOutput { id: id.to_owned(), ..Default::default() };

    // Parse expression
    let document_expression = data.document["expression"].clone();
    let parsed_expression = parse_expression(&document_expression, &data.spec, &mut data.result);
    output.expression = to_json(data.result.expression.as_ref().unwrap_or(&Value::Null));

    // Evaluate expression
    if let Some(parsed) = &parsed_expression {
        evaluate_expression(&data.inputs, &**parsed, &mut data.result);
        output.serialized = to_json(data.result.serialized.as_ref().unwrap_or(&Value::Null));

        // round trip
        let document = to_document(data.result.serialized.as_ref().expect("serialized"));
        let recompiled_expression = parse_expression(&document, &data.spec, &mut data.recompiled);
        if let Some(recompiled) = &recompiled_expression {
            evaluate_expression(&data.inputs, &**recompiled, &mut data.recompiled);
            rewrite_roundtripped_type(
                &data.expected.compiled.serialized_type,
                &mut data.recompiled.compiled.serialized_type,
            );
        }
    }

    // GL JS compares every number to 6 significant figures; colors here also
    // absorb this port's f32 rounding (`color_output_equal`).
    let is_color = data.expected.compiled.serialized_type == "color";
    let compile_ok = data.result.compiled == data.expected.compiled;
    let eval_diff = differing_inputs(data.result.outputs.as_ref(), data.expected.outputs.as_ref(), is_color);
    let eval_ok = compile_ok && eval_diff.as_ref().is_some_and(Vec::is_empty);

    let mut recompile_ok = true;
    let mut round_trip_ok = true;
    let mut serialization_ok = true;
    let mut round_trip_diff = Some(Vec::new());

    if data.expected.compiled.success {
        serialization_ok = compile_ok && deep_equal_opt(&data.result.serialized, &data.expected.serialized);
        recompile_ok = compile_ok && data.recompiled.compiled == data.expected.compiled;
        round_trip_diff = differing_inputs(data.recompiled.outputs.as_ref(), data.expected.outputs.as_ref(), is_color);
        round_trip_ok = recompile_ok && round_trip_diff.as_ref().is_some_and(Vec::is_empty);
    }

    output.passed = compile_ok && eval_ok && recompile_ok && round_trip_ok && serialization_ok;
    if !output.passed
        && compile_ok
        && recompile_ok
        && serialization_ok
        && let (Some(eval_diff), Some(round_trip_diff)) = (eval_diff, round_trip_diff)
    {
        output.failed_inputs = eval_diff;
        output.failed_inputs.extend(round_trip_diff);
        output.failed_inputs.sort_unstable();
        output.failed_inputs.dedup();
    }

    let null = Value::Null;
    if !compile_ok {
        let _ = writeln!(
            output.text,
            "Compiled expression difference:\n{}",
            simple_diff(&compiled_to_value(&data.result.compiled), &compiled_to_value(&data.expected.compiled))
        );
    }

    if compile_ok && !serialization_ok {
        let _ = writeln!(
            output.text,
            "Serialized expression difference:\n{}",
            simple_diff(
                data.expected.serialized.as_ref().unwrap_or(&null),
                data.result.serialized.as_ref().unwrap_or(&null)
            )
        );
    }

    if compile_ok && !recompile_ok {
        let _ = writeln!(
            output.text,
            "Recompiled expression difference:\n{}",
            simple_diff(&compiled_to_value(&data.recompiled.compiled), &compiled_to_value(&data.expected.compiled))
        );
    }

    if compile_ok && !eval_ok {
        let _ = writeln!(
            output.text,
            "Expression outputs difference:\n{}",
            simple_diff(data.expected.outputs.as_ref().unwrap_or(&null), data.result.outputs.as_ref().unwrap_or(&null))
        );
    }

    if recompile_ok && !round_trip_ok {
        let _ = writeln!(
            output.text,
            "Roundtripped through serialize expression outputs difference:\n{}",
            simple_diff(
                data.expected.outputs.as_ref().unwrap_or(&null),
                data.recompiled.outputs.as_ref().unwrap_or(&null)
            )
        );
    }

    output
}
