use std::hash::Hasher;
use std::sync::{Arc, OnceLock};

use crate::conversion::{Convertible, Error};
use crate::expression::{
    EvaluationContext, Expr, Functions, ParsingContext, Type, Value as ExpressionValue, feature_property_keys,
};
use crate::feature::Value;
use crate::property::function::{conversion_errors, get, literal, matchable, number, op, owned};
use crate::wire::optional_expression;

#[derive(Clone, Debug, Default, mln_wire::Wire)]
pub struct Filter {
    #[wire(with = "optional_expression")]
    pub expression: Option<Arc<Expr>>,
    legacy_filter: Option<Value>,
    /// [`Filter::property_keys`], found on first use.
    #[wire(skip)]
    property_keys: OnceLock<Option<Box<[String]>>>,
    /// [`Filter::expression_hash`], found on first use.
    #[wire(skip)]
    expression_hash: OnceLock<u64>,
    /// The hash of [`Filter::equality`]'s operand, found on first use.
    #[wire(skip)]
    operand_hash: OnceLock<Option<u64>>,
}

/// A filter that holds where an operand equals a literal
/// ([`Filter::equality`]).
pub struct Equality<'a> {
    pub operand: &'a Expr,
    pub value: &'a ExpressionValue,
    /// A hash of the operand's structure, alike for equal operands.
    pub operand_hash: u64,
}

impl Filter {
    #[must_use]
    pub fn new(expression: Option<Arc<Expr>>, legacy_filter: Option<Value>) -> Self {
        Filter {
            expression,
            legacy_filter,
            property_keys: OnceLock::new(),
            expression_hash: OnceLock::new(),
            operand_hash: OnceLock::new(),
        }
    }

    pub(crate) fn is_set(&self) -> bool {
        self.expression.is_some() || self.legacy_filter.is_some()
    }

    pub fn evaluate(&self, context: &EvaluationContext<'_>) -> bool {
        let Some(expression) = &self.expression else {
            return true;
        };

        expression.eval_bool(context).unwrap_or(false)
    }

    /// Port-only: the property keys the filter reads, sorted, if they and
    /// the geometry type are all it reads of a feature
    /// ([`feature_property_keys`]). The filter then selects features with
    /// the same geometry type and the same values under those keys alike.
    /// `None` without an expression.
    pub fn property_keys(&self) -> Option<&[String]> {
        let keys = self.property_keys.get_or_init(|| {
            let expression = self.expression.as_ref()?;
            feature_property_keys(expression).map(Vec::into_boxed_slice)
        });
        keys.as_deref()
    }

    /// Port-only: a hash of the expression's structure
    /// ([`Expr::hash_structure`]), so that filters whose expressions
    /// are equal hash alike, whether or not they share them.
    pub fn expression_hash(&self) -> u64 {
        *self.expression_hash.get_or_init(|| {
            let mut state = std::hash::DefaultHasher::new();
            if let Some(expression) = &self.expression {
                expression.hash_structure(&mut state);
            }
            state.finish()
        })
    }

    /// Port-only: the operand and literal of a filter `["==", operand,
    /// literal]`, which selects the features whose operand evaluates to a
    /// value equal to the literal. Filters over one source layer that
    /// compare the same operand can then share its value for each feature.
    pub fn equality(&self) -> Option<Equality<'_>> {
        let (operand, value) = self.expression.as_ref()?.as_equality()?;
        let operand_hash = (*self.operand_hash.get_or_init(|| {
            let mut state = std::hash::DefaultHasher::new();
            operand.hash_structure(&mut state);
            Some(state.finish())
        }))?;
        Some(Equality { operand, value, operand_hash })
    }

    pub(crate) fn serialize(&self) -> Value {
        if let Some(legacy_filter) = &self.legacy_filter {
            legacy_filter.clone()
        } else if let Some(expression) = &self.expression {
            Expr::serialize(expression)
        } else {
            Value::Null
        }
    }
}

impl PartialEq for Filter {
    /// Two filters are equal when their expressions are, whether or not
    /// they share them.
    fn eq(&self, other: &Self) -> bool {
        match (&self.expression, &other.expression) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
            (None, None) => true,
            _ => false,
        }
    }
}

/// Converts a filter: an expression, which parses against `functions`, or a
/// legacy filter, which becomes the expression it stands for.
pub fn convert_filter(value: &Convertible<'_>, functions: &Arc<Functions>, error: &mut Error) -> Option<Filter> {
    let legacy = if is_expression(value) { None } else { Some(legacy_filter_expression(value, error)?) };
    let mut parsing_context = ParsingContext::with_expected(functions, Type::Boolean);
    let parsed = match &legacy {
        Some(legacy) => parsing_context.parse_expression(&Convertible::from(legacy), None),
        None => parsing_context.parse_expression(value, None),
    };
    let Some(parsed) = parsed else {
        error.message =
            if legacy.is_some() { conversion_errors(&parsing_context) } else { parsing_context.get_combined_errors() };
        return None;
    };
    let legacy_filter = legacy.is_some().then(|| owned(value)).filter(|_| !value.is_undefined());
    Some(Filter::new(Some(Arc::new(parsed)), legacy_filter))
}

// This is a port from
// https://github.com/mapbox/mapbox-gl-js/blob/master/src/style-spec/feature_filter/index.js
fn is_expression(filter: &Convertible<'_>) -> bool {
    if filter.to_bool().is_some() {
        return true;
    }
    if !filter.is_array() || filter.array_length() == 0 {
        return false;
    }

    let Some(op) = filter.array_member(0).to_string() else {
        return false;
    };

    if op == "has" {
        if filter.array_length() < 2 {
            return false;
        }
        let operand = filter.array_member(1).to_string();
        operand.is_some_and(|operand| operand != "$id" && operand != "$type")
    } else if op == "!in" || op == "!has" || op == "none" {
        false
    } else if op == "in" {
        let str_ = filter.array_member(1).to_string();

        filter.array_length() >= 3 && (str_.is_none() || filter.array_member(2).is_array())
    } else if op == "==" || op == "!=" || op == ">" || op == ">=" || op == "<" || op == "<=" {
        filter.array_length() != 3 || filter.array_member(1).is_array() || filter.array_member(2).is_array()
    } else if op == "any" || op == "all" {
        for i in 1..filter.array_length() {
            let f = filter.array_member(i);
            if !is_expression(&f) {
                return false;
            }
        }
        true
    } else {
        true
    }
}

fn fail(error: &mut Error, message: &str) -> Option<Value> {
    error.message = message.to_owned();
    None
}

/// What a legacy filter compares: the feature's geometry type, its id or
/// one of its properties.
fn operand(property: &str) -> Value {
    match property {
        "$type" => op("geometry-type", []),
        "$id" => op("id", []),
        key => get(key),
    }
}

/// Whether the feature's `property` equals `value`. A property equals null
/// only where the feature has it.
fn equals(property: &str, value: Value) -> Value {
    if value == Value::Null && property != "$id" && property != "$type" {
        return op("all", [op("has", [property.into()]), op("==", [get(property), Value::Null])]);
    }
    op("==", [operand(property), literal(value)])
}

/// `["==", key, value]` and the other comparisons. An ordering holds only
/// between two numbers or two strings, where an expression would fail.
fn comparison(name: &str, filter: &Convertible<'_>, error: &mut Error) -> Option<Value> {
    let Some(property) = filter.array_member(1).to_string() else {
        return fail(error, "filter property must be a string");
    };
    if filter.array_length() != 3 {
        return fail(error, "filter must compare a property with one value");
    }
    let value = owned(&filter.array_member(2));
    match name {
        "==" => Some(equals(&property, value)),
        "!=" => Some(op("!", [equals(&property, value)])),
        _ if property == "$type" => fail(error, "a geometry type can only be compared for equality"),
        _ => {
            let type_ = match value {
                Value::String(_) => "string",
                _ if number(&value).is_some() => "number",
                _ => return Some(false.into()),
            };
            let operand = operand(&property);
            let is_type = op("==", [op("typeof", [operand.clone()]), type_.into()]);
            Some(op("case", [is_type, op(name, [operand, value]), false.into()]))
        }
    }
}

fn has(filter: &Convertible<'_>, error: &mut Error) -> Option<Value> {
    match filter.array_member(1).to_string().as_deref() {
        None => fail(error, "filter property must be a string"),
        Some("$type") => Some(true.into()),
        Some("$id") => Some(op("!=", [op("id", []), Value::Null])),
        Some(key) => Some(op("has", [key.into()])),
    }
}

fn in_(filter: &Convertible<'_>, error: &mut Error) -> Option<Value> {
    let Some(property) = filter.array_member(1).to_string() else {
        return fail(error, "filter property must be a string");
    };
    let mut values: Vec<Value> = Vec::new();
    for i in 2..filter.array_length() {
        let value = owned(&filter.array_member(i));
        let value = number(&value).map_or(value, Value::Double);
        if !values.contains(&value) {
            values.push(value);
        }
    }
    if values.is_empty() {
        return Some(false.into());
    }
    if matchable(&values) {
        return Some(op("match", [operand(&property), Value::Array(values), true.into(), false.into()]));
    }
    Some(op("any", values.into_iter().map(|value| equals(&property, value))))
}

/// A legacy filter, as the expression the style specification defines it
/// by.
fn legacy_filter_expression(filter: &Convertible<'_>, error: &mut Error) -> Option<Value> {
    if filter.is_undefined() {
        return Some(true.into());
    }
    if !filter.is_array() || filter.array_length() == 0 {
        return fail(error, "filter value must be a non empty array");
    }
    let Some(name) = filter.array_member(0).to_string() else {
        return fail(error, "filter operator must be a string");
    };
    if filter.array_length() <= 1 {
        return Some((name != "any").into());
    }
    let children = |error: &mut Error| -> Option<Vec<Value>> {
        (1..filter.array_length()).map(|i| legacy_filter_expression(&filter.array_member(i), error)).collect()
    };
    match name.as_str() {
        "within" => Some(owned(filter)),
        "==" | "!=" | "<" | ">" | "<=" | ">=" => comparison(&name, filter, error),
        "any" | "all" => Some(op(&name, children(error)?)),
        "none" => Some(op("!", [op("any", children(error)?)])),
        "in" => in_(filter, error),
        "!in" => Some(op("!", [in_(filter, error)?])),
        "has" => has(filter, error),
        "!has" => Some(op("!", [has(filter, error)?])),
        _ => Some(true.into()),
    }
}
