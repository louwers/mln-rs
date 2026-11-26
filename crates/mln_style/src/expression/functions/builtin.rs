//! The style specification's expression functions.

use std::borrow::Cow;

use feature::{Identifier as FeatureIdentifier, Value as MbglValue};
use mln_math::{fmax, fmin};

use super::{Eval, Functions, OverloadSpec, Params, RegisterError, Role};
use crate::color::Color;
use crate::expression::forms::coercion::rgba;
use crate::expression::forms::length::get_unicode_character_offset_on_validated_utf8;
use crate::expression::type_::{self, Type};
use crate::expression::value::{ObjectType, Value, from_mbgl_value_owned, to_string, type_of};
use crate::expression::{
    Dependency, EvalResult, EvaluationContext, EvaluationError, EvaluationResult, Expr, mbgl_to_expression_value,
};
use crate::feature::GeometryTileFeature;
use crate::geometry::FeatureType;
use crate::i18n;

/// Adds the style specification's functions to `functions`, each name's
/// overloads in the order parsing tries them. Fails if `functions` already
/// has one of their names or roles.
pub fn register_builtin_functions(functions: &mut Functions) -> Result<(), RegisterError> {
    use Dependency as D;
    use Type as T;

    let fixed = |types: &[Type]| Params::Fixed(types.to_vec());
    let none = || fixed(&[]);
    let num1 = || fixed(&[T::Number]);
    let num2 = || fixed(&[T::Number, T::Number]);
    let str1 = || fixed(&[T::String]);
    let num_varargs = || Params::Varargs(T::Number);
    let spec = |result: Type, params: Params, dependencies: Dependency, eval: Eval| OverloadSpec {
        result,
        params,
        dependencies,
        role: Role::Plain,
        eval,
    };
    let number = |params: Params, eval| spec(T::Number, params, D::NONE, Eval::Number(eval));
    let with_role = |role: Role, overload: OverloadSpec| OverloadSpec { role, ..overload };

    let mut add = |name: &str, overloads: Vec<OverloadSpec>| functions.add(name, overloads);
    add("e", vec![number(none(), e)])?;
    add("pi", vec![number(none(), pi)])?;
    add("ln2", vec![number(none(), ln2)])?;
    add("typeof", vec![spec(T::String, fixed(&[T::Value]), D::NONE, Eval::String(typeof_))])?;
    add("to-rgba", vec![spec(T::array_n(T::Number, 4), fixed(&[T::Color]), D::NONE, Eval::Value(to_rgba_))])?;
    add(
        "rgba",
        vec![spec(T::Color, fixed(&[T::Number, T::Number, T::Number, T::Number]), D::NONE, Eval::Color(rgba_))],
    )?;
    add("rgb", vec![spec(T::Color, fixed(&[T::Number, T::Number, T::Number]), D::NONE, Eval::Color(rgb))])?;
    add("zoom", vec![with_role(Role::Zoom, spec(T::Number, none(), D::ZOOM, Eval::Number(zoom)))])?;
    add("heatmap-density", vec![spec(T::Number, none(), D::PROPERTY_INPUT, Eval::Number(heatmap_density))])?;
    add("elevation", vec![spec(T::Number, none(), D::ELEVATION, Eval::Number(elevation))])?;
    add("line-progress", vec![spec(T::Number, none(), D::PROPERTY_INPUT, Eval::Number(line_progress))])?;
    add("accumulated", vec![spec(T::Value, none(), D::PROPERTY_INPUT, Eval::Value(accumulated))])?;
    add(
        "has",
        vec![
            with_role(Role::FeatureProperty, spec(T::Boolean, str1(), D::FEATURE, Eval::Boolean(has))),
            spec(T::Boolean, fixed(&[T::String, T::Object]), D::NONE, Eval::Boolean(has_object)),
        ],
    )?;
    add(
        "get",
        vec![
            with_role(Role::FeatureProperty, spec(T::Value, str1(), D::FEATURE, Eval::Value(get))),
            spec(T::Value, fixed(&[T::String, T::Object]), D::NONE, Eval::Value(get_object)),
        ],
    )?;
    add("properties", vec![spec(T::Object, none(), D::FEATURE, Eval::Value(properties))])?;
    add(
        "geometry-type",
        vec![with_role(Role::GeometryType, spec(T::String, none(), D::FEATURE, Eval::String(geometry_type)))],
    )?;
    add("id", vec![with_role(Role::FeatureId, spec(T::Value, none(), D::FEATURE, Eval::Value(id)))])?;
    add("+", vec![number(num_varargs(), add_)])?;
    add("-", vec![number(num2(), sub), number(num1(), neg)])?;
    add("*", vec![number(num_varargs(), mul)])?;
    add("/", vec![number(num2(), div)])?;
    add("%", vec![number(num2(), rem)])?;
    add("^", vec![number(num2(), pow)])?;
    add("sqrt", vec![number(num1(), sqrt)])?;
    add("log10", vec![number(num1(), log10)])?;
    add("ln", vec![number(num1(), ln)])?;
    add("log2", vec![number(num1(), log2)])?;
    add("sin", vec![number(num1(), sin)])?;
    add("cos", vec![number(num1(), cos)])?;
    add("tan", vec![number(num1(), tan)])?;
    add("asin", vec![number(num1(), asin)])?;
    add("acos", vec![number(num1(), acos)])?;
    add("atan", vec![number(num1(), atan)])?;
    add("min", vec![number(num_varargs(), min)])?;
    add("max", vec![number(num_varargs(), max)])?;
    add("round", vec![number(num1(), round)])?;
    add("floor", vec![number(num1(), floor)])?;
    add("ceil", vec![number(num1(), ceil)])?;
    add("abs", vec![number(num1(), abs)])?;
    add("!", vec![spec(T::Boolean, fixed(&[T::Boolean]), D::NONE, Eval::Boolean(not))])?;
    add("is-supported-script", vec![spec(T::Boolean, str1(), D::NONE, Eval::Boolean(is_supported_script))])?;
    add("upcase", vec![spec(T::String, str1(), D::NONE, Eval::String(upcase))])?;
    add("downcase", vec![spec(T::String, str1(), D::NONE, Eval::String(downcase))])?;
    add("concat", vec![spec(T::String, Params::Varargs(T::Value), D::NONE, Eval::String(concat))])?;
    add("split", vec![spec(T::array(T::String), fixed(&[T::String, T::String]), D::NONE, Eval::Value(split))])?;
    add("join", vec![spec(T::String, fixed(&[T::array(T::String), T::String]), D::NONE, Eval::String(join))])?;
    add("resolved-locale", vec![spec(T::String, fixed(&[T::Collator]), D::NONE, Eval::String(resolved_locale))])?;
    add("error", vec![spec(T::Error, str1(), D::NONE, Eval::Value(error))])?;
    add("feature-state", vec![spec(T::Value, str1(), D::FEATURE | D::FEATURE_STATE, Eval::Value(feature_state))])?;
    add(
        "global-state",
        vec![with_role(Role::GlobalState, spec(T::Value, str1(), D::GLOBAL_STATE, Eval::Value(global_state)))],
    )?;
    Ok(())
}

pub(crate) fn feature_unavailable() -> EvaluationError {
    EvaluationError::new("Feature data is unavailable in the current evaluation context.")
}

fn feature<'p>(cx: &EvaluationContext<'p>) -> EvalResult<&'p dyn GeometryTileFeature> {
    cx.feature.ok_or_else(feature_unavailable)
}

/// A feature id as a feature value: `None` for no id.
pub(crate) fn identifier_to_mbgl(id: FeatureIdentifier) -> Option<MbglValue> {
    match id {
        FeatureIdentifier::Null => None,
        FeatureIdentifier::Uint(v) => Some(MbglValue::Uint(v)),
        FeatureIdentifier::Int(v) => Some(MbglValue::Int(v)),
        FeatureIdentifier::Double(v) => Some(MbglValue::Double(v)),
        FeatureIdentifier::String(v) => Some(MbglValue::String(v)),
    }
}

fn identifier_to_expression_value(id: FeatureIdentifier) -> Value {
    match id {
        FeatureIdentifier::Null => Value::Null,
        FeatureIdentifier::Uint(v) => Value::Number(v as f64),
        FeatureIdentifier::Int(v) => Value::Number(v as f64),
        FeatureIdentifier::Double(v) => Value::Number(v),
        FeatureIdentifier::String(v) => Value::String(v),
    }
}

fn feature_type_as_string(t: FeatureType) -> &'static str {
    match t {
        FeatureType::Point => "Point",
        FeatureType::LineString => "LineString",
        FeatureType::Polygon => "Polygon",
        FeatureType::Unknown => "Unknown",
    }
}

// Numbers.

#[expect(clippy::unnecessary_wraps, reason = "every number evaluator has this signature")]
fn e(_: &[Expr], _: &EvaluationContext<'_>) -> EvalResult<f64> {
    Ok(std::f64::consts::E)
}

#[expect(clippy::unnecessary_wraps, reason = "every number evaluator has this signature")]
fn pi(_: &[Expr], _: &EvaluationContext<'_>) -> EvalResult<f64> {
    Ok(std::f64::consts::PI)
}

#[expect(clippy::unnecessary_wraps, reason = "every number evaluator has this signature")]
fn ln2(_: &[Expr], _: &EvaluationContext<'_>) -> EvalResult<f64> {
    Ok(std::f64::consts::LN_2)
}

fn zoom(_: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    match cx.zoom {
        Some(zoom) => Ok(f64::from(zoom)),
        None => Err(EvaluationError::new("The 'zoom' expression is unavailable in the current evaluation context.")),
    }
}

fn heatmap_density(_: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    cx.color_ramp_parameter.ok_or_else(|| {
        EvaluationError::new("The 'heatmap-density' expression is unavailable in the current evaluation context.")
    })
}

/// For color-relief, elevation comes as the color ramp parameter; for 3D
/// terrain, as the elevation. While parsing, 0 lets parsing succeed: the
/// elevation dependency keeps the expression from folding into a constant.
#[expect(clippy::unnecessary_wraps, reason = "every number evaluator has this signature")]
fn elevation(_: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    Ok(cx.color_ramp_parameter.or(cx.elevation.map(f64::from)).unwrap_or(0.0))
}

fn line_progress(_: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    cx.color_ramp_parameter.ok_or_else(|| {
        EvaluationError::new("The 'line-progress' expression is unavailable in the current evaluation context.")
    })
}

fn add_(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    let mut sum = 0.0;
    for arg in args {
        sum += arg.eval_num(cx)?;
    }
    Ok(sum)
}

fn mul(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    let mut product = 1.0;
    for arg in args {
        product *= arg.eval_num(cx)?;
    }
    Ok(product)
}

fn min(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    let mut result = f64::INFINITY;
    for arg in args {
        result = fmin(arg.eval_num(cx)?, result);
    }
    Ok(result)
}

fn max(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    let mut result = f64::NEG_INFINITY;
    for arg in args {
        result = fmax(arg.eval_num(cx)?, result);
    }
    Ok(result)
}

fn sub(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    Ok(args[0].eval_num(cx)? - args[1].eval_num(cx)?)
}

fn neg(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    Ok(-args[0].eval_num(cx)?)
}

fn div(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    Ok(args[0].eval_num(cx)? / args[1].eval_num(cx)?)
}

/// Rust's float `%` is C fmod.
fn rem(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    Ok(args[0].eval_num(cx)? % args[1].eval_num(cx)?)
}

fn pow(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    Ok(args[0].eval_num(cx)?.powf(args[1].eval_num(cx)?))
}

/// Declares functions of one number argument that apply an `f64` method to
/// it.
macro_rules! unary {
    ($($(#[$meta:meta])* $name:ident => $method:ident;)*) => {
        $(
            $(#[$meta])*
            fn $name(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
                Ok(args[0].eval_num(cx)?.$method())
            }
        )*
    };
}

unary! {
    sqrt => sqrt;
    log10 => log10;
    ln => ln;
    log2 => log2;
    sin => sin;
    cos => cos;
    tan => tan;
    asin => asin;
    acos => acos;
    atan => atan;
    /// C `round` rounds half away from zero, as `f64::round` does.
    round => round;
    floor => floor;
    ceil => ceil;
    abs => abs;
}

// Booleans.

fn not(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<bool> {
    Ok(!args[0].eval_bool(cx)?)
}

fn has(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<bool> {
    let key = args[0].eval_str(cx)?;
    Ok(feature(cx)?.get_value(&key).is_some())
}

fn has_object(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<bool> {
    let key = args[0].eval_str(cx)?;
    let object = args[1].evaluate_ref(cx)?;
    let Value::Object(object) = &*object else {
        return Err(EvaluationError::type_mismatch(&Type::Object, &object));
    };
    Ok(object.contains_key(&*key))
}

fn is_supported_script(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<bool> {
    let x = args[0].eval_str(cx)?;
    let Some(font_faces) = cx.font_faces else {
        return Ok(i18n::is_string_in_supported_script(&x));
    };
    Ok(x.chars().all(|chr| i18n::char_in_supported_script(chr) || font_faces.covers(chr)))
}

// Strings.

fn typeof_<'e>(args: &'e [Expr], cx: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
    Ok(Cow::Owned(type_::to_string(&type_of(&*args[0].evaluate_ref(cx)?))))
}

fn geometry_type<'e>(_: &'e [Expr], cx: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
    Ok(Cow::Borrowed(feature_type_as_string(feature(cx)?.get_type())))
}

fn upcase<'e>(args: &'e [Expr], cx: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
    Ok(Cow::Owned(args[0].eval_str(cx)?.to_uppercase()))
}

fn downcase<'e>(args: &'e [Expr], cx: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
    Ok(Cow::Owned(args[0].eval_str(cx)?.to_lowercase()))
}

fn concat<'e>(args: &'e [Expr], cx: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
    let mut s = String::new();
    for arg in args {
        match &*arg.evaluate_ref(cx)? {
            Value::String(part) => s += part,
            other => s += &to_string(other),
        }
    }
    Ok(Cow::Owned(s))
}

fn join<'e>(args: &'e [Expr], cx: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
    let input = args[0].evaluate_ref(cx)?;
    let Value::Array(items) = &*input else {
        return Err(EvaluationError::type_mismatch(&Type::array(Type::String), &input));
    };
    let Some(items) = items.iter().map(Value::as_str).collect::<Option<Vec<&str>>>() else {
        return Err(EvaluationError::type_mismatch(&Type::array(Type::String), &input));
    };
    Ok(Cow::Owned(items.join(&args[1].eval_str(cx)?)))
}

fn resolved_locale<'e>(args: &'e [Expr], cx: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
    let collator = args[0].evaluate_ref(cx)?;
    let Value::Collator(collator_value) = &*collator else {
        return Err(EvaluationError::type_mismatch(&Type::Collator, &collator));
    };
    Ok(Cow::Owned(collator_value.resolved_locale().to_owned()))
}

// Colors.

fn rgba_(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<Color> {
    rgba(args[0].eval_num(cx)?, args[1].eval_num(cx)?, args[2].eval_num(cx)?, args[3].eval_num(cx)?)
}

fn rgb(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<Color> {
    rgba(args[0].eval_num(cx)?, args[1].eval_num(cx)?, args[2].eval_num(cx)?, 1.0)
}

// Other values.

fn to_rgba_(args: &[Expr], cx: &EvaluationContext<'_>) -> EvaluationResult {
    let color = args[0].eval_color(cx)?;
    Ok(Value::Array(color.to_array().iter().map(|v| Value::Number(*v)).collect()))
}

fn accumulated(_: &[Expr], cx: &EvaluationContext<'_>) -> EvaluationResult {
    match cx.accumulated {
        None => {
            Err(EvaluationError::new("The 'accumulated' expression is unavailable in the current evaluation context."))
        }
        Some(v) => Ok(mbgl_to_expression_value(v)),
    }
}

fn get(args: &[Expr], cx: &EvaluationContext<'_>) -> EvaluationResult {
    let key = args[0].eval_str(cx)?;
    Ok(feature(cx)?.get_value(&key).map_or(Value::Null, from_mbgl_value_owned))
}

fn get_object(args: &[Expr], cx: &EvaluationContext<'_>) -> EvaluationResult {
    let key = args[0].eval_str(cx)?;
    let object = args[1].evaluate_ref(cx)?;
    let Value::Object(object) = &*object else {
        return Err(EvaluationError::type_mismatch(&Type::Object, &object));
    };
    Ok(object.get(&*key).cloned().unwrap_or(Value::Null))
}

fn properties(_: &[Expr], cx: &EvaluationContext<'_>) -> EvaluationResult {
    let mut result = ObjectType::new();
    for (k, v) in feature(cx)?.get_properties().iter() {
        result.insert(k.clone(), mbgl_to_expression_value(v));
    }
    Ok(Value::Object(result))
}

fn id(_: &[Expr], cx: &EvaluationContext<'_>) -> EvaluationResult {
    Ok(identifier_to_expression_value(feature(cx)?.get_id()))
}

fn split(args: &[Expr], cx: &EvaluationContext<'_>) -> EvaluationResult {
    let input = args[0].eval_str(cx)?;
    let delimiter = args[1].eval_str(cx)?;
    Ok(Value::Array(split_str(&input, &delimiter)))
}

fn feature_state(args: &[Expr], cx: &EvaluationContext<'_>) -> EvaluationResult {
    let key = args[0].eval_str(cx)?;
    let state = cx.feature_state.and_then(|state| state.get(&*key));
    Ok(state.map_or(Value::Null, mbgl_to_expression_value))
}

fn global_state(args: &[Expr], cx: &EvaluationContext<'_>) -> EvaluationResult {
    let key = args[0].eval_str(cx)?;
    let state = cx.global_state.and_then(|state| state.get(&*key));
    Ok(state.map_or(Value::Null, mbgl_to_expression_value))
}

/// `["error", message]` fails with its message.
fn error(args: &[Expr], cx: &EvaluationContext<'_>) -> EvaluationResult {
    Err(match args[0].eval_str(cx) {
        Ok(message) => EvaluationError::new(message.into_owned()),
        Err(e) => e,
    })
}

fn split_str(input: &str, delimiter: &str) -> Vec<Value> {
    let mut result = Vec::new();
    if delimiter.is_empty() {
        let mut remaining = input;
        while !remaining.is_empty() {
            let character_length = get_unicode_character_offset_on_validated_utf8(remaining, 1);
            result.push(Value::String(remaining[..character_length].to_owned()));
            remaining = &remaining[character_length..];
        }
        return result;
    }
    let mut start = 0;
    loop {
        match input[start..].find(delimiter) {
            None => {
                result.push(Value::String(input[start..].to_owned()));
                break;
            }
            Some(offset) => {
                let position = start + offset;
                result.push(Value::String(input[start..position].to_owned()));
                start = position + delimiter.len();
            }
        }
    }
    result
}
