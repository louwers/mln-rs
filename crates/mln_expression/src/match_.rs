// Ports include/mln/style/expression/match.hpp and
// src/mln/style/expression/match.cpp. (The module is `match_` because
// `match` is a Rust keyword.)

use std::collections::BTreeMap;
use std::sync::Arc;

use feature::Value as MbglValue;
use mln_conversion::Convertible;

use crate::check_subtype::check_subtype;
use crate::expression::{Dependency, EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, downcast};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::Value;

/// A match label type: `int64_t` or `std::string`.
pub trait MatchLabel: Ord + Clone + Send + Sync + 'static {
    /// `Match<T>::evaluate`'s lookup key for an input value, if it can match.
    fn from_input(input: &Value) -> Option<Option<Self>>;
    fn to_mbgl(&self) -> MbglValue;
    fn from_input_type(label: &InputType) -> Self;
}

impl MatchLabel for String {
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
    fn from_input(input: &Value) -> Option<Option<Self>> {
        match input {
            Value::Number(numeric) => {
                // static_cast<int64_t>(std::floor(numeric)) as clang compiles
                // it (cvttsd2si: out of range and NaN give INT64_MIN).
                let floored = numeric.floor();
                let rounded = if floored >= -9223372036854775808.0 && floored < 9223372036854775808.0 {
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

/// `Match<T>::Branches` (C++ `std::unordered_map`; ordered here).
pub type Branches<T> = BTreeMap<T, Arc<dyn Expression>>;

pub struct Match<T: MatchLabel> {
    base: ExpressionBase,
    input: Box<dyn Expression>,
    branches: Branches<T>,
    otherwise: Box<dyn Expression>,
}

impl<T: MatchLabel> Match<T> {
    pub fn new(type_: Type, input: Box<dyn Expression>, branches: Branches<T>, otherwise: Box<dyn Expression>) -> Self {
        let deps = input.dependencies()
            | otherwise.dependencies()
            | branches.values().fold(Dependency::NONE, |a, e| a | e.dependencies());
        Match { base: ExpressionBase::new(Kind::Match, type_, deps), input, branches, otherwise }
    }
}

impl<T: MatchLabel> Expression for Match<T> {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let input_value = self.input.evaluate(params)?;

        match T::from_input(&input_value) {
            None => self.otherwise.evaluate(params),
            Some(None) => self.otherwise.evaluate(params),
            Some(Some(key)) => match self.branches.get(&key) {
                Some(branch) => branch.evaluate(params),
                None => self.otherwise.evaluate(params),
            },
        }
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        visit(&*self.input);
        for branch in self.branches.values() {
            visit(&**branch);
        }
        visit(&*self.otherwise);
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Match {
            if let Some(rhs) = downcast::<Match<T>>(e) {
                return self.input.equals(&*rhs.input)
                    && self.otherwise.equals(&*rhs.otherwise)
                    && self.branches.len() == rhs.branches.len()
                    && self.branches.iter().zip(&rhs.branches).all(|(l, r)| l.0 == r.0 && l.1.equals(&**r.1));
            }
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        let mut result = Vec::new();
        for branch in self.branches.values() {
            result.extend(branch.possible_outputs());
        }
        result.extend(self.otherwise.possible_outputs());
        result
    }

    fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String(self.get_operator()), self.input.serialize()];

        // Sort so serialization has an arbitrary defined order, even though
        // branch order doesn't affect evaluation (the BTreeMap is sorted).

        // Group branches by unique match expression to support condensed
        // serializations of the form [case1, case2, ...] -> matchExpression
        let mut output_lookup: Vec<(*const (), usize)> = Vec::new();
        let mut grouped_by_output: Vec<(&Arc<dyn Expression>, Vec<MbglValue>)> = Vec::new();
        for (label, output) in &self.branches {
            let key = Arc::as_ptr(output) as *const ();
            if let Some((_, index)) = output_lookup.iter().find(|(p, _)| *p == key) {
                // We've seen this expression before, add the label to that
                // output's group
                grouped_by_output[*index].1.push(label.to_mbgl());
            } else {
                // First time seeing this output, add it to the end of the
                // grouped list
                output_lookup.push((key, grouped_by_output.len()));
                grouped_by_output.push((output, vec![label.to_mbgl()]));
            }
        }

        for (output, labels) in grouped_by_output {
            if labels.len() == 1 {
                // Only a single label matches this output expression
                serialized.push(labels.into_iter().next().unwrap());
            } else {
                // Array of literal labels pointing to this output expression
                serialized.push(MbglValue::Array(labels));
            }
            // The output expression itself
            serialized.push(output.serialize());
        }

        serialized.push(self.otherwise.serialize());
        MbglValue::Array(serialized)
    }

    fn get_operator(&self) -> String {
        "match".to_owned()
    }
}

/// `InputType`: `variant<int64_t, std::string>`.
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
            if !Value::is_safe_integer_u64(n) {
                parent_context.error_at(not_safe(), index);
            } else {
                type_ = Some(Type::Number);
                result = Some(InputType::Int(n as i64));
            }
        }
        Some(MbglValue::Int(n)) => {
            if !Value::is_safe_integer_i64(n) {
                parent_context.error_at(not_safe(), index);
            } else {
                type_ = Some(Type::Number);
                result = Some(InputType::Int(n));
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

type ParsedBranches = Vec<(Vec<InputType>, Box<dyn Expression>)>;

fn create<T: MatchLabel>(
    output_type: Type,
    input: Box<dyn Expression>,
    branches: ParsedBranches,
    otherwise: Box<dyn Expression>,
    ctx: &mut ParsingContext,
) -> ParseResult {
    let mut typed_branches = Branches::<T>::new();

    let mut index = 2;

    for (labels, output) in branches {
        let result: Arc<dyn Expression> = Arc::from(output);
        for label in &labels {
            let typed_label = T::from_input_type(label);
            if typed_branches.contains_key(&typed_label) {
                ctx.error_at("Branch labels must be unique.".to_owned(), index);
                return None;
            }
            typed_branches.insert(typed_label, result.clone());
        }

        index += 2;
    }
    Some(Box::new(Match::<T>::new(output_type, input, typed_branches, otherwise)))
}

pub fn parse_match(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
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
    if let Some(expected) = ctx.get_expected() {
        if expected != Type::Value {
            output_type = Some(expected);
        }
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
            output_type = Some(output.get_type().clone());
        }

        branches.push((labels, output));
        i += 2;
    }

    let input = ctx.parse(&value.array_member(1), 1, Some(Type::Value), None)?;

    let otherwise = ctx.parse(&value.array_member(length - 1), length - 1, output_type.clone(), None)?;

    let input_type = input_type.expect("match input type");
    let output_type = output_type.expect("match output type");

    if *input.get_type() != Type::Value {
        if let Some(err) = check_subtype(&input_type, input.get_type()) {
            ctx.error_at(err, 1);
            return None;
        }
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
