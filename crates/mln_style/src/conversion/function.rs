// Quirk kept: the conversion interpolates with ColorSpace::RGB; the legacy
// "colorSpace" member is not read.

use std::cmp::Ordering;
use std::sync::Arc;

use mln_expression::Interpolator;
use mln_expression::case::Case;
use mln_expression::format_expression::{FormatExpression, FormatExpressionSection};
use mln_expression::formatted::Formatted;
use mln_expression::image_expression::ImageExpression;
use mln_expression::match_::{Match, MatchLabel};
use mln_style_types::{ProjectionDefinition, VariableAnchorOffsetCollection};
use mln_util::{Color, ColorSpace, Padding};
use mln_vec_map::VecMap;

use super::{Converter, Convertible, Error, convert};
use crate::expression::{
    Expression, Image, ParsingContext, Step, Stops, Type, Value, ValueConverter, create_interpolate, dsl,
};
use crate::property_expression::PropertyExpression;

const TOKEN_RESERVED_CHARS: &[u8] = b"{}";

/// Whether `source` contains a `{token}`.
#[must_use]
pub fn has_tokens(source: &str) -> bool {
    let bytes = source.as_bytes();
    let end = bytes.len();
    let mut pos = 0;

    while pos != end {
        let Some(offset) = bytes[pos..].iter().position(|&c| c == b'{') else {
            return false;
        };
        let mut brace = pos + offset;
        brace += 1;
        while brace != end && !TOKEN_RESERVED_CHARS.contains(&bytes[brace]) {
            brace += 1;
        }
        if brace != end && bytes[brace] == b'}' {
            return true;
        }
        pos = brace;
    }

    false
}

#[must_use]
pub fn convert_token_string_to_format_expression(source: &str) -> Box<dyn Expression> {
    let text_expression = convert_token_string_to_expression(source);
    let sections = vec![FormatExpressionSection::new(text_expression)];
    Box::new(FormatExpression::new(sections))
}

#[must_use]
pub fn convert_token_string_to_image_expression(source: &str) -> Box<dyn Expression> {
    Box::new(ImageExpression::new(convert_token_string_to_expression(source)))
}

#[must_use]
pub fn convert_token_string_to_expression(source: &str) -> Box<dyn Expression> {
    let mut inputs: Vec<Box<dyn Expression>> = Vec::new();

    // Byte positions; '{' and '}' are ASCII, so every slice is valid UTF-8.
    let bytes = source.as_bytes();
    let end = bytes.len();
    let mut pos = 0;

    while pos != end {
        let brace = bytes[pos..].iter().position(|&c| c == b'{').map_or(end, |o| pos + o);
        if pos != brace {
            inputs.push(dsl::literal_str(&source[pos..brace]));
        }
        pos = brace;
        if pos != end {
            let mut brace = brace + 1;
            while brace != end && !TOKEN_RESERVED_CHARS.contains(&bytes[brace]) {
                brace += 1;
            }
            if brace != end && bytes[brace] == b'}' {
                inputs.push(dsl::get(dsl::literal_str(&source[pos + 1..brace])));
                pos = brace + 1;
            } else {
                inputs.push(dsl::literal_str(&source[pos..brace]));
                pos = brace;
            }
        }
    }

    match inputs.len() {
        0 => dsl::literal_str(source),
        1 => dsl::to_string(inputs.pop().unwrap(), None),
        _ => dsl::concat(inputs),
    }
}

pub fn convert_function_to_expression<T>(
    value: &Convertible<'_>,
    error: &mut Error,
    convert_tokens: bool,
) -> Option<PropertyExpression<T>>
where
    T: Converter + ValueConverter,
{
    let expression = convert_function_to_expression_for_type(&T::expression_type(), value, error, convert_tokens)?;

    let mut default_value: Option<T> = None;

    if let Some(default_value_value) = value.object_member("default") {
        default_value = convert::<T>(&default_value_value, error);
        if default_value.is_none() {
            error.message = format!(r#"wrong type for "default": {}"#, error.message);
            return None;
        }
    }

    Some(PropertyExpression::from_box(expression, default_value))
}

// Ad-hoc Converters for double and int64_t. We should replace float with double
// wholesale, and promote the int64_t Converter to general use (and it should
// check that the input is an integer).
fn convert_double(value: &Convertible<'_>, error: &mut Error) -> Option<f64> {
    let converted = convert::<f32>(value, error)?;
    Some(f64::from(converted))
}

fn convert_int64(value: &Convertible<'_>, error: &mut Error) -> Option<i64> {
    let converted = convert::<f32>(value, error)?;
    // static_cast<int64_t>(float): truncation (`as` saturates where C++ is
    // undefined).
    Some(converted as i64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FunctionType {
    Interval,
    Exponential,
    Categorical,
    Identity,
    Invalid,
}

fn interpolatable(type_: &Type) -> bool {
    match type_ {
        Type::Number
        | Type::Color
        | Type::Padding
        | Type::ProjectionDefinition
        | Type::VariableAnchorOffsetCollection => true,
        Type::Array(array) => {
            // Arrays are interpolatable if they have a fixed size and item type is Number,
            // OR if the item type itself is interpolatable (e.g., Array<Color>)
            if array.n.is_some() && array.item_type == Type::Number {
                return true;
            }
            // Variable-length Number arrays (like line-dasharray) are not interpolatable
            // because interpolating between different dasharray patterns doesn't make semantic sense
            if array.n.is_none() && array.item_type == Type::Number {
                return false;
            }
            interpolatable(&array.item_type)
        }
        _ => false,
    }
}

/// The C++ `type.match([&](const type::Array& arr) { arr.itemType.match([&](const type::ColorType&) {...`
/// blocks: the item type of an array-of-colors type.
fn color_array_item_type(type_: &Type) -> Option<Type> {
    match type_ {
        Type::Array(array) if array.item_type == Type::Color => Some(array.item_type.clone()),
        _ => None,
    }
}

fn convert_literal(
    type_: &Type,
    value: &Convertible<'_>,
    error: &mut Error,
    convert_tokens: bool,
) -> Option<Box<dyn Expression>> {
    match type_ {
        Type::Number => {
            let result = convert::<f32>(value, error)?;
            Some(dsl::literal(Value::Number(f64::from(result))))
        }
        Type::Boolean => {
            let result = convert::<bool>(value, error)?;
            Some(dsl::literal(Value::Bool(result)))
        }
        Type::String => {
            let result = convert::<String>(value, error)?;
            Some(if convert_tokens {
                convert_token_string_to_expression(&result)
            } else {
                dsl::literal(Value::String(result))
            })
        }
        Type::Color => {
            let result = convert::<Color>(value, error)?;
            Some(dsl::literal(Value::Color(result)))
        }
        Type::Padding => {
            let result = convert::<Padding>(value, error)?;
            Some(dsl::literal(Value::Padding(result)))
        }
        Type::ProjectionDefinition => {
            let result = convert::<ProjectionDefinition>(value, error)?;
            Some(dsl::literal(Value::ProjectionDefinition(result)))
        }
        Type::VariableAnchorOffsetCollection => {
            let result = convert::<VariableAnchorOffsetCollection>(value, error)?;
            Some(dsl::literal(Value::VariableAnchorOffsetCollection(result)))
        }
        Type::Array(array) => {
            if !value.is_array() {
                error.message = "value must be an array".to_owned();
                return None;
            }
            if let Some(n) = array.n
                && value.array_length() != n
            {
                error.message = format!("value must be an array of length {n}");
                return None;
            }
            match &array.item_type {
                Type::Number => {
                    let mut result = Vec::with_capacity(value.array_length());
                    for i in 0..value.array_length() {
                        let Some(number) = value.array_member(i).to_number() else {
                            error.message = "value must be an array of numbers".to_owned();
                            return None;
                        };
                        result.push(Value::Number(f64::from(number)));
                    }
                    Some(dsl::literal(Value::Array(result)))
                }
                Type::String => {
                    let mut result = Vec::with_capacity(value.array_length());
                    for i in 0..value.array_length() {
                        let Some(string) = value.array_member(i).to_string() else {
                            error.message = "value must be an array of strings".to_owned();
                            return None;
                        };
                        result.push(Value::String(string));
                    }
                    Some(dsl::literal(Value::Array(result)))
                }
                _ => {
                    debug_assert!(false, "No properties use this type.");
                    None
                }
            }
        }
        Type::Null | Type::Object | Type::Error | Type::Value | Type::Collator => {
            debug_assert!(false, "No properties use this type.");
            None
        }
        Type::Formatted => {
            let result = convert::<String>(value, error)?;
            Some(if convert_tokens {
                convert_token_string_to_format_expression(&result)
            } else {
                dsl::literal(Value::Formatted(Formatted::from_plain(&result)))
            })
        }
        Type::Image => {
            let result = convert::<String>(value, error)?;
            Some(if convert_tokens {
                // Quirk kept: C++ wraps the ImageExpression in a second one.
                Box::new(ImageExpression::new(convert_token_string_to_image_expression(&result)))
            } else {
                dsl::literal(Value::Image(Image::new(result)))
            })
        }
    }
}

/// Stops ordered by `<`, where `emplace` keeps the first value of a key, as
/// C++'s function conversion builds them.
#[derive(Default)]
struct DoubleStops {
    stops: Vec<(f64, Box<dyn Expression>)>,
}

impl DoubleStops {
    fn lower_bound(&self, key: f64) -> usize {
        self.stops.partition_point(|(k, _)| *k < key)
    }

    fn emplace(&mut self, key: f64, value: Box<dyn Expression>) {
        let i = self.lower_bound(key);
        if i < self.stops.len() && key.partial_cmp(&self.stops[i].0) != Some(Ordering::Less) {
            return;
        }
        self.stops.insert(i, (key, value));
    }

    fn into_stops(self) -> Stops {
        let mut stops = Stops::new();
        for (k, v) in self.stops {
            stops.emplace(k, v);
        }
        stops
    }
}

fn convert_stops(
    type_: &Type,
    value: &Convertible<'_>,
    error: &mut Error,
    convert_tokens: bool,
) -> Option<DoubleStops> {
    let Some(stops_value) = value.object_member("stops") else {
        error.message = "function value must specify stops".to_owned();
        return None;
    };
    if !stops_value.is_array() {
        error.message = "function stops must be an array".to_owned();
        return None;
    }
    if stops_value.array_length() == 0 {
        error.message = "function must have at least one stop".to_owned();
        return None;
    }

    // For std::vector<Color> (hillshade color arrays), parse stops as the item type
    // This allows zoom function interpolation of individual colors
    // Other array types (translate, dasharray, etc) use the full type
    let stop_type = color_array_item_type(type_).unwrap_or_else(|| type_.clone());

    let mut stops = DoubleStops::default();
    for i in 0..stops_value.array_length() {
        let stop_value = stops_value.array_member(i);

        if !stop_value.is_array() {
            error.message = "function stop must be an array".to_owned();
            return None;
        }

        if stop_value.array_length() != 2 {
            error.message = "function stop must have two elements".to_owned();
            return None;
        }

        let t = convert::<f32>(&stop_value.array_member(0), error)?;

        let e = convert_literal(&stop_type, &stop_value.array_member(1), error, convert_tokens)?;
        stops.emplace(f64::from(t), e);
    }
    Some(stops)
}

fn omit_first_stop(stops: &mut DoubleStops) {
    let mut min = f64::MAX;
    for s in &stops.stops {
        // std::min(s.first, min)
        min = if min < s.0 { min } else { s.0 };
    }
    // stops.emplace(-inf, std::move(stops[min])); stops.erase(min);
    let i = stops.lower_bound(min);
    if i < stops.stops.len() && min.partial_cmp(&stops.stops[i].0) != Some(Ordering::Less) {
        let (_, value) = stops.stops.remove(i);
        stops.emplace(f64::NEG_INFINITY, value);
    }
}

fn convert_branches<T: Ord>(
    type_: &Type,
    value: &Convertible<'_>,
    error: &mut Error,
    convert_key: fn(&Convertible<'_>, &mut Error) -> Option<T>,
) -> Option<VecMap<T, Box<dyn Expression>>> {
    let Some(stops_value) = value.object_member("stops") else {
        error.message = "function value must specify stops".to_owned();
        return None;
    };

    if !stops_value.is_array() {
        error.message = "function stops must be an array".to_owned();
        return None;
    }

    if stops_value.array_length() == 0 {
        error.message = "function must have at least one stop".to_owned();
        return None;
    }

    let mut stops = VecMap::new();
    for i in 0..stops_value.array_length() {
        let stop_value = stops_value.array_member(i);

        if !stop_value.is_array() {
            error.message = "function stop must be an array".to_owned();
            return None;
        }

        if stop_value.array_length() != 2 {
            error.message = "function stop must have two elements".to_owned();
            return None;
        }

        let t = convert_key(&stop_value.array_member(0), error)?;

        let e = convert_literal(type_, &stop_value.array_member(1), error, false)?;

        stops.entry(t).or_insert(e);
    }

    Some(stops)
}

fn convert_base(value: &Convertible<'_>, error: &mut Error) -> Option<f64> {
    let Some(base_value) = value.object_member("base") else {
        return Some(1.0);
    };

    let Some(base) = base_value.to_number() else {
        error.message = "function base must be a number".to_owned();
        return None;
    };

    Some(f64::from(base))
}

fn step(type_: Type, input: Box<dyn Expression>, stops: DoubleStops) -> Box<dyn Expression> {
    Box::new(Step::new(type_, input, stops.into_stops()))
}

fn interpolate(
    type_: Type,
    interpolator: Interpolator,
    input: Box<dyn Expression>,
    stops: DoubleStops,
) -> Box<dyn Expression> {
    let mut ctx = ParsingContext::new();
    let result = create_interpolate(type_, interpolator, ColorSpace::Rgb, input, stops.into_stops(), &mut ctx);
    // C++ asserts and returns a null expression.
    result.expect("convertFunctionToExpression: createInterpolate failed")
}

/// A `match` on `property` over integer or string labels.
fn categorical<T: MatchLabel>(
    type_: Type,
    property: &str,
    branches: VecMap<T, Box<dyn Expression>>,
    def: Option<Box<dyn Expression>>,
) -> Box<dyn Expression> {
    let converted_branches: VecMap<T, Arc<dyn Expression>> =
        branches.into_iter().map(|(k, v)| (k, Arc::from(v))).collect();
    Box::new(Match::<T>::new(
        type_,
        dsl::get(dsl::literal_str(property)),
        converted_branches,
        def.unwrap_or_else(|| dsl::error("replaced with default")),
    ))
}

fn categorical_bool(
    type_: Type,
    property: &str,
    mut branches: VecMap<bool, Box<dyn Expression>>,
    def: Option<Box<dyn Expression>>,
) -> Box<dyn Expression> {
    let true_case = branches.remove(&true).unwrap_or_else(|| dsl::error("replaced with default"));
    let false_case = branches.remove(&false).unwrap_or_else(|| dsl::error("replaced with default"));

    let converted_branches = vec![
        (dsl::eq(dsl::get(dsl::literal_str(property)), dsl::literal(Value::Bool(true))), true_case),
        (dsl::eq(dsl::get(dsl::literal_str(property)), dsl::literal(Value::Bool(false))), false_case),
    ];

    Box::new(Case::new(type_, converted_branches, def.unwrap_or_else(|| dsl::error("replaced with default"))))
}

fn number_or_default(
    type_: Type,
    get: Box<dyn Expression>,
    expr: Box<dyn Expression>,
    def: Option<Box<dyn Expression>>,
) -> Box<dyn Expression> {
    let Some(def) = def else {
        return expr;
    };

    let branches = vec![(dsl::eq(dsl::compound("typeof", vec![get]), dsl::literal_str("number")), expr)];
    Box::new(Case::new(type_, branches, def))
}

fn convert_interval_function(
    type_: Type,
    value: &Convertible<'_>,
    error: &mut Error,
    make_input: &dyn Fn(bool) -> Box<dyn Expression>,
    def: Option<Box<dyn Expression>>,
    convert_tokens: bool,
) -> Option<Box<dyn Expression>> {
    let mut stops = convert_stops(&type_, value, error, convert_tokens)?;
    omit_first_stop(&mut stops);

    // For std::vector<Color> arrays, create step with item type
    let color_item = color_array_item_type(&type_);
    let is_color_array = color_item.is_some();
    let expr_type = color_item.unwrap_or_else(|| type_.clone());

    let expr = step(expr_type, make_input(true), stops);

    // For color arrays with camera functions, return the step directly
    if is_color_array && def.is_none() {
        return Some(expr);
    }

    Some(number_or_default(type_, make_input(false), expr, def))
}

fn convert_exponential_function(
    type_: Type,
    value: &Convertible<'_>,
    error: &mut Error,
    make_input: &dyn Fn(bool) -> Box<dyn Expression>,
    def: Option<Box<dyn Expression>>,
    convert_tokens: bool,
) -> Option<Box<dyn Expression>> {
    let stops = convert_stops(&type_, value, error, convert_tokens)?;
    let base = convert_base(value, error)?;

    // For std::vector<Color> arrays, create interpolation with item type
    let color_item = color_array_item_type(&type_);
    let is_color_array = color_item.is_some();
    let expr_type = color_item.unwrap_or_else(|| type_.clone());

    let expr = interpolate(expr_type, dsl::exponential(base), make_input(true), stops);

    // For color arrays with camera functions, return the interpolation directly
    if is_color_array && def.is_none() {
        return Some(expr);
    }

    Some(number_or_default(type_, make_input(false), expr, def))
}

fn convert_categorical_function(
    type_: Type,
    value: &Convertible<'_>,
    err: &mut Error,
    property: &str,
    def: Option<Box<dyn Expression>>,
) -> Option<Box<dyn Expression>> {
    let Some(stops_value) = value.object_member("stops") else {
        err.message = "function value must specify stops".to_owned();
        return None;
    };

    if !stops_value.is_array() {
        err.message = "function stops must be an array".to_owned();
        return None;
    }

    if stops_value.array_length() == 0 {
        err.message = "function must have at least one stop".to_owned();
        return None;
    }

    let first = stops_value.array_member(0);

    if !first.is_array() {
        err.message = "function stop must be an array".to_owned();
        return None;
    }

    if first.array_length() != 2 {
        err.message = "function stop must have two elements".to_owned();
        return None;
    }

    if first.array_member(0).to_bool().is_some() {
        let branches = convert_branches::<bool>(&type_, value, err, convert::<bool>)?;
        return Some(categorical_bool(type_, property, branches, def));
    }

    if first.array_member(0).to_number().is_some() {
        let branches = convert_branches::<i64>(&type_, value, err, convert_int64)?;
        return Some(categorical(type_, property, branches, def));
    }

    if first.array_member(0).to_string().is_some() {
        let branches = convert_branches::<String>(&type_, value, err, convert::<String>)?;
        return Some(categorical(type_, property, branches, def));
    }

    err.message = "stop domain value must be a number, string, or boolean".to_owned();
    None
}

/// The inner `std::map<T, std::unique_ptr<Expression>>` of a composite
/// function (`emplace` keeps the first value of a key).
trait InnerStops: Default {
    type Key;
    fn emplace(&mut self, key: Self::Key, value: Box<dyn Expression>);
}

impl InnerStops for DoubleStops {
    type Key = f64;
    fn emplace(&mut self, key: f64, value: Box<dyn Expression>) {
        DoubleStops::emplace(self, key, value);
    }
}

impl<K: Ord> InnerStops for VecMap<K, Box<dyn Expression>> {
    type Key = K;
    fn emplace(&mut self, key: K, value: Box<dyn Expression>) {
        self.entry(key).or_insert(value);
    }
}

type KeyConverter<'a, K> = &'a dyn Fn(&Convertible<'_>, &mut Error) -> Option<K>;

fn composite<M: InnerStops>(
    type_: Type,
    value: &Convertible<'_>,
    error: &mut Error,
    convert_key: KeyConverter<'_, M::Key>,
    make_inner_expression: &dyn Fn(&Type, f64, M) -> Box<dyn Expression>,
) -> Option<Box<dyn Expression>> {
    let base = convert_base(value, error)?;

    // Checked by caller.
    let stops_value = value.object_member("stops").expect("checked by caller");
    debug_assert!(stops_value.is_array());

    // std::map<float, std::map<T, std::unique_ptr<Expression>>>
    let mut map: Vec<(f32, M)> = Vec::new();

    for i in 0..stops_value.array_length() {
        let stop_value = stops_value.array_member(i);

        if !stop_value.is_array() {
            error.message = "function stop must be an array".to_owned();
            return None;
        }

        if stop_value.array_length() != 2 {
            error.message = "function stop must have two elements".to_owned();
            return None;
        }

        let stop_input = stop_value.array_member(0);

        if !stop_input.is_object() {
            error.message = "stop input must be an object".to_owned();
            return None;
        }

        let Some(zoom_value) = stop_input.object_member("zoom") else {
            error.message = "stop input must specify zoom".to_owned();
            return None;
        };

        let Some(source_value) = stop_input.object_member("value") else {
            error.message = "stop input must specify value".to_owned();
            return None;
        };

        let z = convert::<f32>(&zoom_value, error)?;

        let d = convert_key(&source_value, error)?;

        let r = convert_literal(&type_, &stop_value.array_member(1), error, false)?;

        // map[*z].emplace(*d, std::move(*r));
        let index = map.partition_point(|(k, _)| *k < z);
        if index >= map.len() || z < map[index].0 {
            map.insert(index, (z, M::default()));
        }
        map[index].1.emplace(d, r);
    }

    let mut stops = DoubleStops::default();

    for (z, inner) in map {
        stops.emplace(f64::from(z), make_inner_expression(&type_, base, inner));
    }

    if interpolatable(&type_) {
        Some(interpolate(type_, dsl::linear(), dsl::zoom(), stops))
    } else {
        Some(step(type_, dsl::zoom(), stops))
    }
}

pub fn convert_function_to_expression_for_type(
    type_: &Type,
    value: &Convertible<'_>,
    err: &mut Error,
    convert_tokens: bool,
) -> Option<Box<dyn Expression>> {
    if !value.is_object() {
        err.message = "function must be an object".to_owned();
        return None;
    }

    let mut function_type = FunctionType::Invalid;

    match value.object_member("type") {
        None => {
            function_type = if interpolatable(type_) { FunctionType::Exponential } else { FunctionType::Interval };
        }
        Some(type_value) => {
            if let Some(string) = type_value.to_string() {
                if string == "interval" {
                    function_type = FunctionType::Interval;
                }
                if string == "exponential" && interpolatable(type_) {
                    function_type = FunctionType::Exponential;
                }
                if string == "categorical" {
                    function_type = FunctionType::Categorical;
                }
                if string == "identity" {
                    function_type = FunctionType::Identity;
                }
            }
        }
    }

    // `defaultExpr()`: the literal "default", or none. Like the C++ lambda
    // it can leave a message in `err` without failing.
    let default_expr = |err: &mut Error| -> Option<Box<dyn Expression>> {
        let member = value.object_member("default")?;
        convert_literal(type_, &member, err, false)
    };

    if value.object_member("property").is_none() {
        // Camera function.
        let zoom_input = |_: bool| dsl::zoom();
        return match function_type {
            FunctionType::Interval => {
                let def = default_expr(err);
                convert_interval_function(type_.clone(), value, err, &zoom_input, def, convert_tokens)
            }
            FunctionType::Exponential => {
                let def = default_expr(err);
                convert_exponential_function(type_.clone(), value, err, &zoom_input, def, convert_tokens)
            }
            _ => {
                err.message = "unsupported function type".to_owned();
                None
            }
        };
    }

    let Some(property_value) = value.object_member("property") else {
        err.message = "function must specify property".to_owned();
        return None;
    };

    let Some(property) = property_value.to_string() else {
        err.message = "function property must be a string".to_owned();
        return None;
    };

    if function_type == FunctionType::Identity {
        let get = || dsl::get(dsl::literal_str(&property));
        return match type_ {
            Type::String => Some(dsl::string(get(), default_expr(err))),
            Type::Number => Some(dsl::number(get(), default_expr(err))),
            Type::Boolean => Some(dsl::boolean(get(), default_expr(err))),
            Type::Color => Some(dsl::to_color(get(), default_expr(err))),
            Type::Padding => Some(dsl::to_padding(get(), default_expr(err))),
            Type::ProjectionDefinition => Some(dsl::to_projection_definition(get(), default_expr(err))),
            Type::VariableAnchorOffsetCollection => Some(dsl::to_variable_anchor_offset(get(), default_expr(err))),
            Type::Array(_) => Some(dsl::assertion(type_.clone(), get(), default_expr(err))),
            Type::Formatted => Some(dsl::to_formatted(get(), default_expr(err))),
            Type::Image => Some(dsl::to_image(get(), default_expr(err))),
            _ => {
                debug_assert!(false, "No properties use this type.");
                None
            }
        };
    }

    let Some(stops_value) = value.object_member("stops") else {
        err.message = "function value must specify stops".to_owned();
        return None;
    };

    if !stops_value.is_array() {
        err.message = "function stops must be an array".to_owned();
        return None;
    }

    if stops_value.array_length() == 0 {
        err.message = "function must have at least one stop".to_owned();
        return None;
    }

    let first = stops_value.array_member(0);

    if !first.is_array() {
        err.message = "function stop must be an array".to_owned();
        return None;
    }

    if first.array_length() != 2 {
        err.message = "function stop must have two elements".to_owned();
        return None;
    }

    let stop = first.array_member(0);

    let get_property = |coerce: bool| -> Box<dyn Expression> {
        if coerce {
            dsl::number(dsl::get(dsl::literal_str(&property)), None)
        } else {
            dsl::get(dsl::literal_str(&property))
        }
    };

    if stop.is_object() {
        // Composite function.
        let Some(source_value) = stop.object_member("value") else {
            err.message = "stop must specify value".to_owned();
            return None;
        };

        // The C++ inner lambdas call defaultExpr() (writing to `err`) once
        // per zoom stop, after every failure point of the conversion, so the
        // messages they leave are never observable; a scratch error matches.
        let default_for_inner = || -> Option<Box<dyn Expression>> {
            let mut scratch = Error::default();
            default_expr(&mut scratch)
        };

        if source_value.to_bool().is_some() {
            return if function_type == FunctionType::Categorical {
                composite::<VecMap<bool, Box<dyn Expression>>>(
                    type_.clone(),
                    value,
                    err,
                    &|v, e| convert::<bool>(v, e),
                    &|type_inner, _, stops| categorical_bool(type_inner.clone(), &property, stops, default_for_inner()),
                )
            } else {
                err.message = "unsupported function type".to_owned();
                None
            };
        }

        if source_value.to_number().is_some() {
            return match function_type {
                FunctionType::Interval => {
                    composite::<DoubleStops>(type_.clone(), value, err, &convert_double, &|type_inner, _, mut stops| {
                        omit_first_stop(&mut stops);
                        number_or_default(
                            type_.clone(),
                            get_property(false),
                            step(type_inner.clone(), get_property(true), stops),
                            default_for_inner(),
                        )
                    })
                }
                FunctionType::Exponential => {
                    composite::<DoubleStops>(type_.clone(), value, err, &convert_double, &|type_inner, base, stops| {
                        number_or_default(
                            type_.clone(),
                            get_property(false),
                            interpolate(type_inner.clone(), dsl::exponential(base), get_property(true), stops),
                            default_for_inner(),
                        )
                    })
                }
                FunctionType::Categorical => composite::<VecMap<i64, Box<dyn Expression>>>(
                    type_.clone(),
                    value,
                    err,
                    &convert_int64,
                    &|type_inner, _, stops| categorical(type_inner.clone(), &property, stops, default_for_inner()),
                ),
                _ => {
                    err.message = "unsupported function type".to_owned();
                    None
                }
            };
        }

        if source_value.to_string().is_some() {
            return if function_type == FunctionType::Categorical {
                composite::<VecMap<String, Box<dyn Expression>>>(
                    type_.clone(),
                    value,
                    err,
                    &|v, e| convert::<String>(v, e),
                    &|type_inner, _, stops| categorical(type_inner.clone(), &property, stops, default_for_inner()),
                )
            } else {
                err.message = "unsupported function type".to_owned();
                None
            };
        }

        err.message = "stop domain value must be a number, string, or boolean".to_owned();
        None
    } else {
        // Source function.
        match function_type {
            FunctionType::Interval => {
                let def = default_expr(err);
                convert_interval_function(type_.clone(), value, err, &get_property, def, false)
            }
            FunctionType::Exponential => {
                let def = default_expr(err);
                convert_exponential_function(type_.clone(), value, err, &get_property, def, false)
            }
            FunctionType::Categorical => {
                let def = default_expr(err);
                convert_categorical_function(type_.clone(), value, err, &property, def)
            }
            _ => {
                err.message = "unsupported function type".to_owned();
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Ports TEST(StyleConversion, TokenStrings) from
    // test/style/conversion/function.test.cpp.
    use super::*;

    // gtest: StyleConversion.TokenStrings
    #[test]
    fn token_strings() {
        use dsl::{concat, get, literal_str};

        assert!(!has_tokens(""));
        assert!(!has_tokens("{"));
        assert!(!has_tokens("{token"));
        assert!(has_tokens("{token}"));
        assert!(has_tokens("token {token}"));
        assert!(has_tokens("{token} {token}"));

        let eq = |a: Box<dyn Expression>, b: Box<dyn Expression>| a.equals(b.as_ref());
        assert!(eq(convert_token_string_to_expression("{token}"), dsl::to_string(get(literal_str("token")), None)));
        assert!(eq(
            convert_token_string_to_expression("token {token}"),
            concat(vec![literal_str("token "), get(literal_str("token"))])
        ));
        assert!(eq(
            convert_token_string_to_expression("{token} token"),
            concat(vec![get(literal_str("token")), literal_str(" token")])
        ));
        assert!(eq(
            convert_token_string_to_expression("{token} {token}"),
            concat(vec![get(literal_str("token")), literal_str(" "), get(literal_str("token"))])
        ));
        assert!(eq(
            convert_token_string_to_expression("{token} {token"),
            concat(vec![get(literal_str("token")), literal_str(" "), literal_str("{token")])
        ));
        assert!(eq(
            convert_token_string_to_expression("{token {token}"),
            concat(vec![literal_str("{token "), get(literal_str("token"))])
        ));
    }
}
