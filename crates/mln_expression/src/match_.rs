// The module is `match_` because `match` is a Rust keyword.

use std::borrow::Cow;

use feature::Value as MbglValue;
use mln_conversion::Convertible;
use mln_vec_map::VecMap;

use crate::check_subtype::check_subtype;
use crate::expression::{EvalResult, EvaluationContext, Expr, Node, Output};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::Value;

/// A match label type: `int64_t` or `std::string`.
pub trait MatchLabel: Ord + Clone + Send + Sync + 'static {
    /// Wraps a label map of this type.
    #[doc(hidden)]
    fn wrap(labels: VecMap<Self, usize>) -> Labels;
    /// `Match<T>::evaluate`'s lookup key for an input value, if it can match.
    fn from_input(input: &Value) -> Option<Option<Self>>;
    fn to_mbgl(&self) -> MbglValue;
    fn from_input_type(label: &InputType) -> Self;
}

impl MatchLabel for String {
    fn wrap(labels: VecMap<Self, usize>) -> Labels {
        Labels::String(labels)
    }
    fn from_input(input: &Value) -> Option<Option<Self>> {
        match input {
            Value::String(s) => Some(Some(s.clone())),
            _ => None,
        }
    }
    fn to_mbgl(&self) -> MbglValue {
        MbglValue::String(self.clone())
    }
    fn from_input_type(label: &InputType) -> Self {
        match label {
            InputType::String(s) => s.clone(),
            InputType::Int(_) => panic!("string label expected"),
        }
    }
}

impl MatchLabel for i64 {
    fn wrap(labels: VecMap<Self, usize>) -> Labels {
        Labels::Int(labels)
    }
    fn from_input(input: &Value) -> Option<Option<Self>> {
        match input {
            Value::Number(numeric) => {
                // static_cast<int64_t>(std::floor(numeric)) as clang compiles
                // it (cvttsd2si: out of range and NaN give INT64_MIN).
                let floored = numeric.floor();
                let rounded = if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&floored) {
                    floored as i64
                } else {
                    i64::MIN
                };
                if *numeric == rounded as f64 { Some(Some(rounded)) } else { Some(None) }
            }
            _ => None,
        }
    }
    fn to_mbgl(&self) -> MbglValue {
        MbglValue::Int(*self)
    }
    fn from_input_type(label: &InputType) -> Self {
        match label {
            InputType::Int(i) => *i,
            InputType::String(_) => panic!("integer label expected"),
        }
    }
}

/// The index of each label's output, by label.
#[derive(Clone, PartialEq)]
#[doc(hidden)]
pub enum Labels {
    Int(VecMap<i64, usize>),
    String(VecMap<String, usize>),
}

/// `["match", input, label, output, ..., otherwise]`.
#[derive(Clone, PartialEq)]
pub struct Match {
    type_: Type,
    input: Expr,
    labels: Labels,
    outputs: Box<[Expr]>,
    otherwise: Expr,
}

impl Match {
    /// A match of `input` with one output per label.
    #[must_use]
    pub fn new_expr<T: MatchLabel>(type_: Type, input: Expr, branches: VecMap<T, Expr>, otherwise: Expr) -> Expr {
        let mut labels = VecMap::new();
        let mut outputs = Vec::with_capacity(branches.len());
        for (label, output) in branches {
            labels.insert(label, outputs.len());
            outputs.push(output);
        }
        Match::with_labels(type_, input, T::wrap(labels), outputs, otherwise)
    }

    fn with_labels(type_: Type, input: Expr, labels: Labels, outputs: Vec<Expr>, otherwise: Expr) -> Expr {
        Expr::new(Node::Match(Box::new(Match { type_, input, labels, outputs: outputs.into(), otherwise })))
    }

    pub(crate) fn get_type(&self) -> &Type {
        &self.type_
    }

    /// The output that the input selects.
    fn select(&self, params: &EvaluationContext<'_>) -> EvalResult<&Expr> {
        let index = match &self.labels {
            Labels::Int(labels) => {
                let key = if self.input.get_type() == Type::Number {
                    int_label(self.input.eval_num(params)?)
                } else {
                    match &*self.input.evaluate_ref(params)? {
                        Value::Number(n) => int_label(*n),
                        _ => None,
                    }
                };
                key.and_then(|key| labels.get(&key))
            }
            Labels::String(labels) => {
                let key: Option<Cow<'_, str>> = if self.input.get_type() == Type::String {
                    Some(self.input.eval_str(params)?)
                } else {
                    match self.input.evaluate(params)? {
                        Value::String(s) => Some(Cow::Owned(s)),
                        _ => None,
                    }
                };
                key.and_then(|key| labels.get(&*key))
            }
        };
        Ok(index.map_or(&self.otherwise, |&i| &self.outputs[i]))
    }

    pub(crate) fn eval<'e, T: Output<'e>>(&'e self, params: &EvaluationContext<'_>) -> EvalResult<T> {
        T::eval(self.select(params)?, params)
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        visit(&self.input);
        self.each_output(visit);
    }

    /// Visits each output, the fallback last.
    pub(crate) fn each_output<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        self.outputs.iter().for_each(&mut *visit);
        visit(&self.otherwise);
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String("match".to_owned()), self.input.serialize()];

        // Each output once, after the labels that select it, in the order of
        // its first label (labels are sorted, so the order is defined).
        let mut grouped: Vec<(usize, Vec<MbglValue>)> = Vec::new();
        let mut group = |label: MbglValue, output: usize| match grouped.iter_mut().find(|(o, _)| *o == output) {
            Some((_, labels)) => labels.push(label),
            None => grouped.push((output, vec![label])),
        };
        match &self.labels {
            Labels::Int(labels) => labels.iter().for_each(|(label, &output)| group(label.to_mbgl(), output)),
            Labels::String(labels) => labels.iter().for_each(|(label, &output)| group(label.to_mbgl(), output)),
        }

        for (output, mut labels) in grouped {
            if labels.len() == 1 {
                serialized.push(labels.pop().expect("a label"));
            } else {
                serialized.push(MbglValue::Array(labels));
            }
            serialized.push(self.outputs[output].serialize());
        }

        serialized.push(self.otherwise.serialize());
        MbglValue::Array(serialized)
    }
}

/// The integer label a number input selects: none unless it is an integer.
fn int_label(numeric: f64) -> Option<i64> {
    i64::from_input(&Value::Number(numeric)).flatten()
}

/// A match label: an integer or a string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputType {
    Int(i64),
    String(String),
}

fn parse_input_value(
    input: &Convertible<'_>,
    parent_context: &mut ParsingContext,
    index: usize,
    input_type: &mut Option<Type>,
) -> Option<InputType> {
    let mut result: Option<InputType> = None;
    let mut type_: Option<Type> = None;

    let not_safe = || format!("Branch labels must be integers no larger than {}.", Value::max_safe_integer());

    match input.to_value() {
        Some(MbglValue::Uint(n)) => {
            if Value::is_safe_integer_u64(n) {
                type_ = Some(Type::Number);
                result = Some(InputType::Int(n as i64));
            } else {
                parent_context.error_at(not_safe(), index);
            }
        }
        Some(MbglValue::Int(n)) => {
            if Value::is_safe_integer_i64(n) {
                type_ = Some(Type::Number);
                result = Some(InputType::Int(n));
            } else {
                parent_context.error_at(not_safe(), index);
            }
        }
        Some(MbglValue::Double(n)) => {
            if !Value::is_safe_integer_f64(n) {
                parent_context.error_at(not_safe(), index);
            } else if n != n.floor() {
                parent_context.error_at("Numeric branch labels must be integer values.".to_owned(), index);
            } else {
                type_ = Some(Type::Number);
                result = Some(InputType::Int(n as i64));
            }
        }
        Some(MbglValue::String(s)) => {
            type_ = Some(Type::String);
            result = Some(InputType::String(s));
        }
        Some(_) | None => {
            parent_context.error_at("Branch labels must be numbers or strings.".to_owned(), index);
        }
    }

    let Some(type_) = type_ else {
        return result;
    };

    if let Some(it) = input_type {
        if let Some(err) = check_subtype(it, &type_) {
            parent_context.error_at(err, index);
            return None;
        }
    } else {
        *input_type = Some(type_);
    }

    result
}

type ParsedBranches = Vec<(Vec<InputType>, Expr)>;

fn create<T: MatchLabel>(
    output_type: Type,
    input: Expr,
    branches: ParsedBranches,
    otherwise: Expr,
    ctx: &mut ParsingContext,
) -> ParseResult {
    let mut labels = VecMap::<T, usize>::new();
    let mut outputs = Vec::with_capacity(branches.len());

    let mut index = 2;

    for (branch_labels, output) in branches {
        for label in &branch_labels {
            let typed_label = T::from_input_type(label);
            if labels.contains_key(&typed_label) {
                ctx.error_at("Branch labels must be unique.".to_owned(), index);
                return None;
            }
            labels.insert(typed_label, outputs.len());
        }
        outputs.push(output);

        index += 2;
    }
    Some(Match::with_labels(output_type, input, T::wrap(labels), outputs, otherwise))
}

pub(crate) fn parse_match(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
    debug_assert!(value.is_array());
    let length = value.array_length();
    if length < 5 {
        ctx.error(format!("Expected at least 4 arguments, but found only {}.", length - 1));
        return None;
    }

    // Expect odd-length array: ["match", input, 2 * (n pairs)..., otherwise]
    if length % 2 != 1 {
        ctx.error("Expected an even number of arguments.".to_owned());
        return None;
    }

    let mut input_type: Option<Type> = None;
    let mut output_type: Option<Type> = None;
    if let Some(expected) = ctx.get_expected()
        && expected != Type::Value
    {
        output_type = Some(expected);
    }

    let mut branches: ParsedBranches = Vec::with_capacity((length - 3) / 2);
    let mut i = 2;
    while i + 1 < length {
        let label = value.array_member(i);

        let mut labels = Vec::new();
        // Match pair inputs are provided as either a literal value or a raw
        // JSON array of string / number / boolean values.
        if label.is_array() {
            let group_length = label.array_length();
            if group_length == 0 {
                ctx.error_at("Expected at least one branch label.".to_owned(), i);
                return None;
            }

            labels.reserve(group_length);
            for j in 0..group_length {
                let input_value = parse_input_value(&label.array_member(j), ctx, i, &mut input_type)?;
                labels.push(input_value);
            }
        } else {
            let input_value = parse_input_value(&label, ctx, i, &mut input_type)?;
            labels.push(input_value);
        }

        let output = ctx.parse(&value.array_member(i + 1), i + 1, output_type.clone(), None)?;

        if output_type.is_none() {
            output_type = Some(output.get_type());
        }

        branches.push((labels, output));
        i += 2;
    }

    let input = ctx.parse(&value.array_member(1), 1, Some(Type::Value), None)?;

    let otherwise = ctx.parse(&value.array_member(length - 1), length - 1, output_type.clone(), None)?;

    let input_type = input_type.expect("match input type");
    let output_type = output_type.expect("match output type");

    let actual_input_type = input.get_type();
    if actual_input_type != Type::Value
        && let Some(err) = check_subtype(&input_type, &actual_input_type)
    {
        ctx.error_at(err, 1);
        return None;
    }

    match input_type {
        Type::Number => create::<i64>(output_type, input, branches, otherwise, ctx),
        Type::String => create::<String>(output_type, input, branches, otherwise, ctx),
        _ => {
            // unreachable: inputType is set by parseInputValue(), which only
            // accepts string and (integer) numeric values.
            debug_assert!(false);
            None
        }
    }
}
