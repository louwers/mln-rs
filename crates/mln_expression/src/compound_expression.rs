// The compound expressions: the operators the style specification defines by
// their signatures. Each signature states its parameter and result types and
// names its operator ([`Op`]), which typed evaluation matches on. The
// registry keeps the overloads of a name in the order parsing tries them.

use std::borrow::Cow;
use std::sync::LazyLock;

use feature::{Identifier as FeatureIdentifier, Value as MbglValue};
use mln_conversion::Convertible;
use mln_math::{fmax, fmin};
use mln_util::geometry::FeatureType;
use mln_util::{Color, i18n};

use crate::check_subtype::check_subtype;
use crate::expression::{
    Dependency, EvalResult, EvaluationContext, EvaluationError, EvaluationResult, Expr, Node, Output,
    mbgl_to_expression_value,
};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::utf8_op_helpers::{get_unicode_character_offset_on_validated_utf8, unicode_length_on_validated_utf8};
use crate::util::rgba;
use crate::value::{ObjectType, Value, from_mbgl_value_owned, to_string, type_of};

/// The parameter list of a signature: fixed types or `VarargsType`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Params {
    Fixed(Vec<Type>),
    Varargs(Type),
}

/// The operator of a compound expression signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    E,
    Pi,
    Ln2,
    Typeof,
    ToRgba,
    Rgba,
    Rgb,
    Zoom,
    HeatmapDensity,
    Elevation,
    LineProgress,
    Accumulated,
    Has,
    HasObject,
    Get,
    GetObject,
    Properties,
    GeometryType,
    Id,
    Add,
    Sub,
    Neg,
    Mul,
    Div,
    Rem,
    Pow,
    Sqrt,
    Log10,
    Ln,
    Log2,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Min,
    Max,
    Round,
    Floor,
    Ceil,
    Abs,
    Not,
    IsSupportedScript,
    Upcase,
    Downcase,
    Concat,
    Split,
    Join,
    ResolvedLocale,
    Error,
    FeatureState,
    GlobalState,
    FilterEq,
    FilterIdEq,
    FilterTypeEq,
    FilterLt,
    FilterIdLt,
    FilterGt,
    FilterIdGt,
    FilterLe,
    FilterIdLe,
    FilterGe,
    FilterIdGe,
    FilterHas,
    FilterHasId,
    FilterTypeIn,
    FilterIdIn,
    FilterIn,
}

pub(crate) struct SignatureBase {
    pub(crate) result: Type,
    pub(crate) params: Params,
    pub(crate) name: &'static str,
    pub(crate) dependencies: Dependency,
    pub(crate) op: Op,
}

fn sig(name: &'static str, result: Type, params: Params, dependencies: Dependency, op: Op) -> SignatureBase {
    SignatureBase { result, params, name, dependencies, op }
}

fn fixed(types: &[Type]) -> Params {
    Params::Fixed(types.to_vec())
}

static REGISTRY: LazyLock<Vec<SignatureBase>> = LazyLock::new(|| {
    use Dependency as D;
    use Type as T;
    let s = T::String;
    let n = T::Number;
    let b = T::Boolean;
    let none = || fixed(&[]);
    let num1 = || fixed(std::slice::from_ref(&T::Number));
    let num2 = || fixed(&[T::Number, T::Number]);
    let str1 = || fixed(std::slice::from_ref(&T::String));
    let key_value = || fixed(&[T::String, T::Value]);
    let mut registry = vec![
        sig("e", n.clone(), none(), D::NONE, Op::E),
        sig("pi", n.clone(), none(), D::NONE, Op::Pi),
        sig("ln2", n.clone(), none(), D::NONE, Op::Ln2),
        sig("typeof", s.clone(), fixed(&[T::Value]), D::NONE, Op::Typeof),
        sig("to-rgba", T::array_n(T::Number, 4), fixed(&[T::Color]), D::NONE, Op::ToRgba),
        sig("rgba", T::Color, fixed(&[n.clone(), n.clone(), n.clone(), n.clone()]), D::NONE, Op::Rgba),
        sig("rgb", T::Color, fixed(&[n.clone(), n.clone(), n.clone()]), D::NONE, Op::Rgb),
        sig("zoom", n.clone(), none(), D::ZOOM, Op::Zoom),
        sig("heatmap-density", n.clone(), none(), D::NONE, Op::HeatmapDensity),
        sig("elevation", n.clone(), none(), D::ELEVATION, Op::Elevation),
        sig("line-progress", n.clone(), none(), D::NONE, Op::LineProgress),
        sig("accumulated", T::Value, none(), D::NONE, Op::Accumulated),
        sig("has", b.clone(), str1(), D::FEATURE, Op::Has),
        sig("has", b.clone(), fixed(&[s.clone(), T::Object]), D::FEATURE, Op::HasObject),
        sig("get", T::Value, str1(), D::FEATURE, Op::Get),
        sig("get", T::Value, fixed(&[s.clone(), T::Object]), D::FEATURE, Op::GetObject),
        sig("properties", T::Object, none(), D::FEATURE, Op::Properties),
        sig("geometry-type", s.clone(), none(), D::FEATURE, Op::GeometryType),
        sig("id", T::Value, none(), D::FEATURE, Op::Id),
        sig("+", n.clone(), Params::Varargs(n.clone()), D::NONE, Op::Add),
        sig("-", n.clone(), num2(), D::NONE, Op::Sub),
        sig("-", n.clone(), num1(), D::NONE, Op::Neg),
        sig("*", n.clone(), Params::Varargs(n.clone()), D::NONE, Op::Mul),
        sig("/", n.clone(), num2(), D::NONE, Op::Div),
        sig("%", n.clone(), num2(), D::NONE, Op::Rem),
        sig("^", n.clone(), num2(), D::NONE, Op::Pow),
    ];
    for (name, op) in [
        ("sqrt", Op::Sqrt),
        ("log10", Op::Log10),
        ("ln", Op::Ln),
        ("log2", Op::Log2),
        ("sin", Op::Sin),
        ("cos", Op::Cos),
        ("tan", Op::Tan),
        ("asin", Op::Asin),
        ("acos", Op::Acos),
        ("atan", Op::Atan),
    ] {
        registry.push(sig(name, n.clone(), num1(), D::NONE, op));
    }
    registry.extend([
        sig("min", n.clone(), Params::Varargs(n.clone()), D::NONE, Op::Min),
        sig("max", n.clone(), Params::Varargs(n.clone()), D::NONE, Op::Max),
        sig("round", n.clone(), num1(), D::NONE, Op::Round),
        sig("floor", n.clone(), num1(), D::NONE, Op::Floor),
        sig("ceil", n.clone(), num1(), D::NONE, Op::Ceil),
        sig("abs", n.clone(), num1(), D::NONE, Op::Abs),
        sig("!", b.clone(), fixed(&[T::Boolean]), D::NONE, Op::Not),
        sig("is-supported-script", b.clone(), str1(), D::NONE, Op::IsSupportedScript),
        sig("upcase", s.clone(), str1(), D::NONE, Op::Upcase),
        sig("downcase", s.clone(), str1(), D::NONE, Op::Downcase),
        sig("concat", s.clone(), Params::Varargs(T::Value), D::NONE, Op::Concat),
        sig("split", T::array(s.clone()), fixed(&[s.clone(), s.clone()]), D::NONE, Op::Split),
        sig("join", s.clone(), fixed(&[T::array(s.clone()), s.clone()]), D::NONE, Op::Join),
        sig("resolved-locale", s.clone(), fixed(&[T::Collator]), D::NONE, Op::ResolvedLocale),
        sig("error", T::Error, str1(), D::NONE, Op::Error),
        sig("feature-state", T::Value, str1(), D::FEATURE, Op::FeatureState),
        sig("global-state", T::Value, str1(), D::GLOBAL_STATE, Op::GlobalState),
        // Legacy filters
        sig("filter-==", b.clone(), key_value(), D::FEATURE, Op::FilterEq),
        sig("filter-id-==", b.clone(), fixed(&[T::Value]), D::FEATURE, Op::FilterIdEq),
        sig("filter-type-==", b.clone(), str1(), D::FEATURE, Op::FilterTypeEq),
        sig("filter-<", b.clone(), key_value(), D::FEATURE, Op::FilterLt),
        sig("filter-id-<", b.clone(), fixed(&[T::Value]), D::FEATURE, Op::FilterIdLt),
        sig("filter->", b.clone(), key_value(), D::FEATURE, Op::FilterGt),
        sig("filter-id->", b.clone(), fixed(&[T::Value]), D::FEATURE, Op::FilterIdGt),
        sig("filter-<=", b.clone(), key_value(), D::FEATURE, Op::FilterLe),
        sig("filter-id-<=", b.clone(), fixed(&[T::Value]), D::FEATURE, Op::FilterIdLe),
        sig("filter->=", b.clone(), key_value(), D::FEATURE, Op::FilterGe),
        sig("filter-id->=", b.clone(), fixed(&[T::Value]), D::FEATURE, Op::FilterIdGe),
        sig("filter-has", b.clone(), str1(), D::FEATURE, Op::FilterHas),
        sig("filter-has-id", b.clone(), none(), D::FEATURE, Op::FilterHasId),
        sig("filter-type-in", b.clone(), Params::Varargs(s.clone()), D::FEATURE, Op::FilterTypeIn),
        sig("filter-id-in", b.clone(), Params::Varargs(T::Value), D::FEATURE, Op::FilterIdIn),
        sig("filter-in", b, Params::Varargs(T::Value), D::FEATURE, Op::FilterIn),
    ]);
    registry
});

/// The signatures registered under `name`, in definition order.
fn definitions(name: &str) -> Vec<&'static SignatureBase> {
    REGISTRY.iter().filter(|s| s.name == name).collect()
}

pub(crate) fn feature_unavailable() -> EvaluationError {
    EvaluationError::new("Feature data is unavailable in the current evaluation context.")
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

fn mbgl_as_double(value: &MbglValue) -> Option<f64> {
    match *value {
        MbglValue::Double(v) => Some(v),
        MbglValue::Uint(v) => Some(v as f64),
        MbglValue::Int(v) => Some(v as f64),
        _ => None,
    }
}

fn id_as_double(id: &FeatureIdentifier) -> Option<f64> {
    match *id {
        FeatureIdentifier::Double(v) => Some(v),
        FeatureIdentifier::Uint(v) => Some(v as f64),
        FeatureIdentifier::Int(v) => Some(v as f64),
        _ => None,
    }
}

/// The comparison of a legacy ordering filter.
fn ordering_holds(op: Op, ordering: Option<std::cmp::Ordering>) -> bool {
    let Some(ordering) = ordering else {
        return false;
    };
    match op {
        Op::FilterLt | Op::FilterIdLt => ordering.is_lt(),
        Op::FilterGt | Op::FilterIdGt => ordering.is_gt(),
        Op::FilterLe | Op::FilterIdLe => ordering.is_le(),
        Op::FilterGe | Op::FilterIdGe => ordering.is_ge(),
        _ => unreachable!("not an ordering filter: {op:?}"),
    }
}

/// An operator with a name and typed arguments.
#[derive(Clone)]
pub(crate) struct CompoundExpression {
    signature: &'static SignatureBase,
    args: Box<[Expr]>,
}

impl PartialEq for CompoundExpression {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.signature, other.signature) && self.args == other.args
    }
}

impl CompoundExpression {
    #[must_use]
    pub(crate) fn get_parameter_count(&self) -> Option<usize> {
        match &self.signature.params {
            Params::Varargs(_) => None,
            Params::Fixed(p) => Some(p.len()),
        }
    }

    /// The signature the arguments matched.
    #[must_use]
    pub(crate) fn signature(&self) -> &'static SignatureBase {
        self.signature
    }

    #[must_use]
    pub(crate) fn args(&self) -> &[Expr] {
        &self.args
    }

    pub(crate) fn exists(name: &str) -> bool {
        REGISTRY.iter().any(|s| s.name == name)
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String(self.signature.name.to_owned())];
        serialized.extend(self.args.iter().map(Expr::serialize));
        MbglValue::Array(serialized)
    }

    /// The error of the "error" operator.
    fn error(&self, params: &EvaluationContext<'_>) -> EvaluationError {
        match self.args[0].eval_str(params) {
            Ok(message) => EvaluationError::new(message.into_owned()),
            Err(e) => e,
        }
    }

    fn feature<'p>(
        params: &EvaluationContext<'p>,
    ) -> EvalResult<&'p dyn mln_tile_id::geometry_tile_data::GeometryTileFeature> {
        params.feature.ok_or_else(feature_unavailable)
    }

    pub(crate) fn eval_num(&self, params: &EvaluationContext<'_>) -> EvalResult<f64> {
        let a = &self.args;
        let num = |i: usize| a[i].eval_num(params);
        Ok(match self.signature.op {
            Op::E => std::f64::consts::E,
            Op::Pi => std::f64::consts::PI,
            Op::Ln2 => std::f64::consts::LN_2,
            Op::Zoom => match params.zoom {
                Some(zoom) => f64::from(zoom),
                None => {
                    return Err(EvaluationError::new(
                        "The 'zoom' expression is unavailable in the current evaluation context.",
                    ));
                }
            },
            Op::HeatmapDensity => params.color_ramp_parameter.ok_or_else(|| {
                EvaluationError::new(
                    "The 'heatmap-density' expression is unavailable in the current evaluation context.",
                )
            })?,
            // For color-relief, elevation comes as the color ramp parameter;
            // for 3D terrain, as the elevation. While parsing, 0 lets parsing
            // succeed: the elevation dependency keeps the expression from
            // folding into a constant.
            Op::Elevation => params.color_ramp_parameter.or(params.elevation.map(f64::from)).unwrap_or(0.0),
            Op::LineProgress => params.color_ramp_parameter.ok_or_else(|| {
                EvaluationError::new("The 'line-progress' expression is unavailable in the current evaluation context.")
            })?,
            Op::Add => {
                let mut sum = 0.0;
                for arg in a {
                    sum += arg.eval_num(params)?;
                }
                sum
            }
            Op::Mul => {
                let mut product = 1.0;
                for arg in a {
                    product *= arg.eval_num(params)?;
                }
                product
            }
            Op::Min => {
                let mut result = f64::INFINITY;
                for arg in a {
                    result = fmin(arg.eval_num(params)?, result);
                }
                result
            }
            Op::Max => {
                let mut result = f64::NEG_INFINITY;
                for arg in a {
                    result = fmax(arg.eval_num(params)?, result);
                }
                result
            }
            Op::Sub => num(0)? - num(1)?,
            Op::Neg => -num(0)?,
            Op::Div => {
                let (x, y) = (num(0)?, num(1)?);
                if y == 0.0 {
                    if x == 0.0 {
                        f64::NAN
                    } else if x > 0.0 {
                        f64::INFINITY
                    } else if x < 0.0 {
                        f64::NEG_INFINITY
                    } else {
                        x / y
                    }
                } else {
                    x / y
                }
            }
            // Rust's float `%` is C fmod.
            Op::Rem => num(0)? % num(1)?,
            Op::Pow => num(0)?.powf(num(1)?),
            Op::Sqrt => num(0)?.sqrt(),
            Op::Log10 => num(0)?.log10(),
            Op::Ln => num(0)?.ln(),
            Op::Log2 => num(0)?.log2(),
            Op::Sin => num(0)?.sin(),
            Op::Cos => num(0)?.cos(),
            Op::Tan => num(0)?.tan(),
            Op::Asin => num(0)?.asin(),
            Op::Acos => num(0)?.acos(),
            Op::Atan => num(0)?.atan(),
            // C `round` rounds half away from zero, as f64::round does.
            Op::Round => num(0)?.round(),
            Op::Floor => num(0)?.floor(),
            Op::Ceil => num(0)?.ceil(),
            Op::Abs => num(0)?.abs(),
            Op::Error => return Err(self.error(params)),
            _ => return f64::from_value(self.evaluate(params)?),
        })
    }

    pub(crate) fn eval_bool(&self, params: &EvaluationContext<'_>) -> EvalResult<bool> {
        let a = &self.args;
        let op = self.signature.op;
        Ok(match op {
            Op::Not => !a[0].eval_bool(params)?,
            Op::Has => {
                let key = a[0].eval_str(params)?;
                Self::feature(params)?.get_value(&key).is_some()
            }
            Op::HasObject => {
                let key = a[0].eval_str(params)?;
                let object = a[1].evaluate_ref(params)?;
                let Value::Object(object) = &*object else { unreachable!("object argument") };
                object.contains_key(&*key)
            }
            Op::IsSupportedScript => is_supported_script(params, &a[0].eval_str(params)?),
            Op::FilterEq => {
                let key = a[0].eval_str(params)?;
                let value = a[1].evaluate_ref(params)?;
                let feature = params.feature.expect("feature");
                feature.get_value(&key).is_some_and(|rhs| *value == from_mbgl_value_owned(rhs))
            }
            Op::FilterIdEq => {
                let value = a[0].evaluate_ref(params)?;
                let feature = params.feature.expect("feature");
                *value == identifier_to_expression_value(feature.get_id())
            }
            Op::FilterTypeEq => {
                let type_ = a[0].eval_str(params)?;
                params.feature.is_some_and(|feature| feature_type_as_string(feature.get_type()) == type_)
            }
            Op::FilterLt | Op::FilterGt | Op::FilterLe | Op::FilterGe => {
                // A legacy comparison filter compares a number to a numeric
                // feature value and a string to a string one.
                let key = a[0].eval_str(params)?;
                let value = a[1].evaluate_ref(params)?;
                let feature = params.feature.expect("feature");
                match &*value {
                    Value::Number(value) => feature
                        .get_value(&key)
                        .and_then(|v| mbgl_as_double(&v))
                        .is_some_and(|rhs| ordering_holds(op, rhs.partial_cmp(value))),
                    Value::String(value) => match feature.get_value(&key) {
                        Some(MbglValue::String(rhs)) => ordering_holds(op, Some(rhs.as_str().cmp(value))),
                        _ => false,
                    },
                    _ => false,
                }
            }
            Op::FilterIdLt | Op::FilterIdGt | Op::FilterIdLe | Op::FilterIdGe => {
                let value = a[0].evaluate_ref(params)?;
                let id = params.feature.expect("feature").get_id();
                match &*value {
                    Value::Number(value) => {
                        id_as_double(&id).is_some_and(|rhs| ordering_holds(op, rhs.partial_cmp(value)))
                    }
                    Value::String(value) => match id {
                        FeatureIdentifier::String(rhs) => ordering_holds(op, Some(rhs.as_str().cmp(value))),
                        _ => false,
                    },
                    _ => false,
                }
            }
            Op::FilterHas => {
                let key = a[0].eval_str(params)?;
                params.feature.expect("feature").get_value(&key).is_some()
            }
            Op::FilterHasId => !matches!(params.feature.expect("feature").get_id(), FeatureIdentifier::Null),
            Op::FilterTypeIn => {
                let t = feature_type_as_string(params.feature.expect("feature").get_type());
                let mut found = false;
                for arg in a {
                    found |= arg.eval_str(params)? == t;
                }
                found
            }
            Op::FilterIdIn => {
                let mut values = Vec::with_capacity(a.len());
                for arg in a {
                    values.push(arg.evaluate_ref(params)?);
                }
                let id = identifier_to_expression_value(params.feature.expect("feature").get_id());
                values.iter().any(|v| **v == id)
            }
            Op::FilterIn => {
                let mut values = Vec::with_capacity(a.len());
                for arg in a {
                    values.push(arg.evaluate_ref(params)?);
                }
                if values.len() < 2 {
                    return Ok(false);
                }
                let Value::String(key) = &*values[0] else { unreachable!("string key") };
                let value = params.feature.expect("feature").get_value(key).map(from_mbgl_value_owned);
                value.is_some_and(|value| values[1..].iter().any(|v| **v == value))
            }
            Op::Error => return Err(self.error(params)),
            _ => return bool::from_value(self.evaluate(params)?),
        })
    }

    pub(crate) fn eval_str<'e>(&'e self, params: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
        let a = &self.args;
        Ok(Cow::Owned(match self.signature.op {
            Op::Typeof => type_::to_string(&type_of(&*a[0].evaluate_ref(params)?)),
            Op::GeometryType => return Ok(Cow::Borrowed(feature_type_as_string(Self::feature(params)?.get_type()))),
            Op::Upcase => a[0].eval_str(params)?.to_uppercase(),
            Op::Downcase => a[0].eval_str(params)?.to_lowercase(),
            Op::Concat => {
                let mut s = String::new();
                for arg in a {
                    match &*arg.evaluate_ref(params)? {
                        Value::String(part) => s += part,
                        other => s += &to_string(other),
                    }
                }
                s
            }
            Op::Join => {
                let input = a[0].evaluate_ref(params)?;
                let Value::Array(input) = &*input else { unreachable!("array argument") };
                let input: Vec<&str> = input.iter().map(|v| v.as_str().expect("string item")).collect();
                input.join(&a[1].eval_str(params)?)
            }
            Op::ResolvedLocale => {
                let collator = a[0].evaluate_ref(params)?;
                let Value::Collator(collator) = &*collator else { unreachable!("collator argument") };
                collator.resolved_locale().to_owned()
            }
            Op::Error => return Err(self.error(params)),
            _ => return Cow::from_value(self.evaluate(params)?),
        }))
    }

    pub(crate) fn eval_color(&self, params: &EvaluationContext<'_>) -> EvalResult<Color> {
        let a = &self.args;
        let num = |i: usize| a[i].eval_num(params);
        match self.signature.op {
            Op::Rgba => rgba(num(0)?, num(1)?, num(2)?, num(3)?),
            Op::Rgb => rgba(num(0)?, num(1)?, num(2)?, 1.0),
            Op::Error => Err(self.error(params)),
            _ => Color::from_value(self.evaluate(params)?),
        }
    }

    pub(crate) fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        match self.signature.result {
            Type::Number => return self.eval_num(params).map(Value::Number),
            Type::Boolean => return self.eval_bool(params).map(Value::Bool),
            Type::String => return self.eval_str(params).map(|s| Value::String(s.into_owned())),
            Type::Color => return self.eval_color(params).map(Value::Color),
            _ => {}
        }
        let a = &self.args;
        match self.signature.op {
            Op::ToRgba => {
                let color = a[0].eval_color(params)?;
                Ok(Value::Array(color.to_array().iter().map(|v| Value::Number(*v)).collect()))
            }
            Op::Accumulated => match params.accumulated {
                None => Err(EvaluationError::new(
                    "The 'accumulated' expression is unavailable in the current evaluation context.",
                )),
                Some(v) => Ok(mbgl_to_expression_value(v)),
            },
            Op::Get => {
                let key = a[0].eval_str(params)?;
                Ok(Self::feature(params)?.get_value(&key).map_or(Value::Null, from_mbgl_value_owned))
            }
            Op::GetObject => {
                let key = a[0].eval_str(params)?;
                let object = a[1].evaluate_ref(params)?;
                let Value::Object(object) = &*object else { unreachable!("object argument") };
                Ok(object.get(&*key).cloned().unwrap_or(Value::Null))
            }
            Op::Properties => {
                let mut result = ObjectType::new();
                for (k, v) in Self::feature(params)?.get_properties() {
                    result.insert(k.clone(), mbgl_to_expression_value(v));
                }
                Ok(Value::Object(result))
            }
            Op::Id => Ok(identifier_to_expression_value(Self::feature(params)?.get_id())),
            Op::Split => {
                let input = a[0].eval_str(params)?;
                let delimiter = a[1].eval_str(params)?;
                Ok(Value::Array(split(&input, &delimiter)))
            }
            Op::FeatureState => {
                let key = a[0].eval_str(params)?;
                let state = params.feature_state.and_then(|state| state.get(&*key));
                Ok(state.map_or(Value::Null, mbgl_to_expression_value))
            }
            Op::GlobalState => {
                let key = a[0].eval_str(params)?;
                let state = params.global_state.and_then(|state| state.get(&*key));
                Ok(state.map_or(Value::Null, mbgl_to_expression_value))
            }
            Op::Error => Err(self.error(params)),
            op => unreachable!("{op:?} has a typed result"),
        }
    }
}

fn split(input: &str, delimiter: &str) -> Vec<Value> {
    let mut result = Vec::new();
    if delimiter.is_empty() {
        let mut remaining = input;
        while !remaining.is_empty() {
            let character_length = get_unicode_character_offset_on_validated_utf8(remaining, 1);
            result.push(Value::String(remaining[..character_length].to_owned()));
            remaining = &remaining[character_length..];
        }
        let _ = unicode_length_on_validated_utf8(input);
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

fn is_supported_script(params: &EvaluationContext<'_>, x: &str) -> bool {
    let Some(font_faces) = params.font_faces.filter(|faces| !faces.is_empty()) else {
        return i18n::is_string_in_supported_script(x);
    };
    x.encode_utf16().all(|chr| {
        i18n::char_in_supported_script(chr)
            || font_faces
                .iter()
                .any(|face| face.ranges.iter().any(|range| u32::from(chr) >= range.0 && u32::from(chr) <= range.1))
    })
}

fn expected_types_error(definitions: &[&'static SignatureBase], args: &[Expr]) -> String {
    // Only used if there are no overloads with matching number of args
    let mut available_overloads: Vec<String> = Vec::new();
    let mut overloads: Vec<String> = Vec::new();
    for signature in definitions {
        match &signature.params {
            Params::Varargs(t) => {
                overloads.push(format!("({})", type_::to_string(t)));
            }
            Params::Fixed(params) => {
                let overload = format!("({})", params.iter().map(type_::to_string).collect::<Vec<_>>().join(", "));
                if params.len() == args.len() {
                    overloads.push(overload);
                } else {
                    available_overloads.push(overload);
                }
            }
        }
    }

    let signatures = if overloads.is_empty() { available_overloads.join(" | ") } else { overloads.join(" | ") };

    let actual_types = args.iter().map(|arg| type_::to_string(&arg.get_type())).collect::<Vec<_>>().join(", ");

    format!("Expected arguments of type {signatures}, but found ({actual_types}) instead.")
}

fn create_compound_expression_from(
    definitions: &[&'static SignatureBase],
    args: Vec<Expr>,
    ctx: &mut ParsingContext,
) -> ParseResult {
    let mut signature_context = ParsingContext::with_key(ctx.get_key());

    for signature in definitions {
        signature_context.clear_errors();

        match &signature.params {
            Params::Fixed(params) => {
                if params.len() != args.len() {
                    signature_context.error(format!(
                        "Expected {} arguments, but found {} instead.",
                        params.len(),
                        args.len()
                    ));
                    continue;
                }

                for (i, arg) in args.iter().enumerate() {
                    if let Some(err) = check_subtype(&params[i], &arg.get_type()) {
                        signature_context.error_at(err, i + 1);
                    }
                }
            }
            Params::Varargs(param_type) => {
                for (i, arg) in args.iter().enumerate() {
                    if let Some(err) = check_subtype(param_type, &arg.get_type()) {
                        signature_context.error_at(err, i + 1);
                    }
                }
            }
        }

        if !signature_context.has_errors() {
            return Some(Expr::new(Node::Compound(Box::new(CompoundExpression { signature, args: args.into() }))));
        }
    }

    if definitions.len() == 1 {
        ctx.append_errors(&signature_context);
    } else {
        ctx.error(expected_types_error(definitions, &args));
    }

    None
}

pub(crate) fn parse_compound_expression(name: &str, value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
    debug_assert!(value.is_array() && value.array_length() > 0);

    let definitions = definitions(name);
    if definitions.is_empty() {
        ctx.error_at(
            format!(r#"Unknown expression "{name}". If you wanted a literal array, use ["literal", [...]]."#),
            0,
        );
        return None;
    }

    let length = value.array_length();

    // Per the style specification, the "global-state" property name must be
    // a string literal, not the result of a sub-expression.
    if name == "global-state" && length == 2 && value.array_member(1).to_string().is_none() {
        ctx.error_at("Global state property must be a string literal.".to_owned(), 1);
        return None;
    }

    // Each failed overload parses its arguments, so two overloads of one arity
    // would make parsing a nested chain of them exponential in its depth.
    let mut overload_matched_arity = false;
    for signature in &definitions {
        let matches = match &signature.params {
            Params::Varargs(_) => true,
            Params::Fixed(p) => p.len() == length - 1,
        };
        if matches {
            debug_assert!(!overload_matched_arity);
            overload_matched_arity = true;

            // First parse all the args, potentially coercing to the types
            // expected by this overload.
            ctx.clear_errors();
            let mut arg_parse_failed = false;
            let mut args = Vec::with_capacity(length - 1);

            for i in 1..length {
                let expected = match &signature.params {
                    Params::Varargs(t) => t.clone(),
                    Params::Fixed(params) => params[i - 1].clone(),
                };

                let Some(parsed) = ctx.parse(&value.array_member(i), i, Some(expected), None) else {
                    arg_parse_failed = true;
                    break;
                };
                args.push(parsed);
            }
            // When the args couldn't be coerced to this overload's expected
            // types, move on to the next one.
            if !arg_parse_failed {
                let parse_with_args = create_compound_expression_from(&definitions, args, ctx);
                if parse_with_args.is_some() {
                    return parse_with_args;
                }
            }
        }
    }
    // The overload that takes this many arguments failed, and its errors say
    // why. Parsing the arguments again would double the work at every level.
    if overload_matched_arity {
        return None;
    }

    // No overload takes this many arguments. Parse the arguments without
    // expected types just for the error message
    ctx.clear_errors();
    let mut args = Vec::with_capacity(length - 1);

    for i in 1..length {
        let parsed = ctx.parse(&value.array_member(i), i, None, None)?;
        args.push(parsed);
    }

    ctx.error(expected_types_error(&definitions, &args));

    None
}

pub fn create_compound_expression(name: &str, args: Vec<Expr>, ctx: &mut ParsingContext) -> ParseResult {
    create_compound_expression_from(&definitions(name), args, ctx)
}
