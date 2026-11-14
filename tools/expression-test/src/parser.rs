// The C++ runner reads the fixtures with rapidjson's full-precision parser
// (correctly rounded, like serde_json).

use std::path::{Path, PathBuf};

use feature::{Feature, Identifier, Value, ValueObject};
use mln_conversion::Convertible;
use mln_expression::type_::{self, Type};
use mln_expression::{Expression, ParsingContext, is_feature_constant, is_zoom_constant};
use mln_tile_id::CanonicalTileID;
use mln_vec_map::VecSet;

pub(crate) struct Input {
    pub(crate) zoom: Option<f32>,
    pub(crate) heatmap_density: Option<f64>,
    pub(crate) canonical: Option<CanonicalTileID>,
    pub(crate) available_images: VecSet<String>,
    pub(crate) feature: Feature<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Compiled {
    pub(crate) success: bool,
    pub(crate) is_feature_constant: bool,
    pub(crate) is_zoom_constant: bool,
    pub(crate) serialized_type: String,
    pub(crate) errors: Vec<Value>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TestResult {
    pub(crate) compiled: Compiled,
    pub(crate) expression: Option<Value>,
    pub(crate) outputs: Option<Value>,
    pub(crate) serialized: Option<Value>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PropertySpec {
    pub(crate) type_: String,
    pub(crate) value: String,
    pub(crate) length: usize,
    pub(crate) is_data_driven: bool,
    pub(crate) expression: Option<Value>,
}

pub(crate) struct TestData {
    pub(crate) inputs: Vec<Input>,
    pub(crate) expected: TestResult,
    pub(crate) result: TestResult,
    pub(crate) recompiled: TestResult,
    pub(crate) spec: Option<PropertySpec>,
    pub(crate) document: serde_json::Value,
}

pub(crate) struct Ignore {
    pub(crate) id: String,
    pub(crate) reason: String,
}

pub(crate) type Ignores = Vec<Ignore>;

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    let text = std::fs::read_to_string(path).map_err(|_| format!("Unable to open file {}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub(crate) fn json_to_value(jsvalue: &serde_json::Value) -> Option<Value> {
    match jsvalue {
        serde_json::Value::Null => Some(Value::Null),
        serde_json::Value::Array(a) => Some(Value::Array(a.iter().filter_map(json_to_value).collect())),
        serde_json::Value::Object(o) => {
            let mut value_map = ValueObject::new();
            for (k, v) in o {
                if let Some(value) = json_to_value(v) {
                    value_map.entry(k.clone()).or_insert(value);
                }
            }
            Some(Value::Object(value_map))
        }
        v => Convertible::from(v).to_value(),
    }
}

fn string_to_type(t: &str) -> Type {
    match t {
        "string" | "number-format" => Type::String,
        "number" => Type::Number,
        "boolean" => Type::Boolean,
        "object" => Type::Object,
        "color" => Type::Color,
        "value" => Type::Value,
        "formatted" => Type::Formatted,
        "resolvedImage" => Type::Image,
        _ => {
            // Should not reach.
            debug_assert!(false, "unknown type {t}");
            Type::Null
        }
    }
}

fn to_expression_type(spec: &PropertySpec) -> Option<Type> {
    if spec.type_ == "array" {
        let item_type = if spec.value.is_empty() { Type::Value } else { string_to_type(&spec.value) };
        if spec.length != 0 {
            return Some(Type::array_n(item_type, spec.length));
        }
        return Some(Type::array(item_type));
    }

    if spec.type_ == "enum" {
        return Some(Type::String);
    }

    if spec.type_.is_empty() { None } else { Some(string_to_type(&spec.type_)) }
}

fn as_str(v: &serde_json::Value) -> String {
    v.as_str().expect("string").to_owned()
}

fn parse_compiled(compiled: &serde_json::Value, data: &mut TestData) {
    let result = as_str(&compiled["result"]);
    data.expected.compiled.success = result == "success";

    if let Some(v) = compiled.get("isFeatureConstant") {
        data.expected.compiled.is_feature_constant = v.as_bool().expect("bool");
    }

    if let Some(v) = compiled.get("isZoomConstant") {
        data.expected.compiled.is_zoom_constant = v.as_bool().expect("bool");
    }

    if let Some(v) = compiled.get("type") {
        data.expected.compiled.serialized_type = as_str(v);
    }

    if let Some(errors) = compiled.get("errors") {
        for error_val in errors.as_array().expect("errors array") {
            let mut error_map = ValueObject::new();
            error_map.insert("key".to_owned(), Value::String(as_str(&error_val["key"])));
            error_map.insert("error".to_owned(), Value::String(as_str(&error_val["error"])));
            data.expected.compiled.errors.push(Value::Object(error_map));
        }
    }
}

fn parse_expected(expected: &serde_json::Value, data: &mut TestData) {
    parse_compiled(&expected["compiled"], data);

    // set outputs
    if let Some(outputs) = expected.get("outputs") {
        data.expected.outputs = json_to_value(outputs);
    }

    // set serialized
    if let Some(serialized) = expected.get("serialized") {
        data.expected.serialized = json_to_value(serialized);
    }
}

fn parse_property_spec(property_spec: &serde_json::Value, data: &mut TestData) {
    let mut spec = PropertySpec::default();

    if let Some(v) = property_spec.get("type") {
        spec.type_ = as_str(v);
    }

    if let Some(v) = property_spec.get("value") {
        spec.value = as_str(v);
    }

    if let Some(v) = property_spec.get("length") {
        spec.length = v.as_f64().expect("number") as usize;
    }

    if property_spec.get("property-type").is_some() {
        spec.is_data_driven = true;
    }

    if let Some(v) = property_spec.get("expression") {
        spec.expression = json_to_value(v);
    }

    data.spec = Some(spec);
}

fn convert_identifier(json: &serde_json::Value) -> Identifier {
    match json {
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Identifier::Uint(u)
            } else if let Some(i) = n.as_i64() {
                Identifier::Int(i)
            } else {
                Identifier::Double(n.as_f64().unwrap())
            }
        }
        serde_json::Value::String(s) => Identifier::String(s.clone()),
        _ => Identifier::Null,
    }
}

fn parse_inputs(inputs_value: &serde_json::Value, data: &mut TestData) -> Result<(), String> {
    for input in inputs_value.as_array().expect("inputs array") {
        let input = input.as_array().expect("input pair");

        // Parse evaluation context, zoom.
        let evaluation_context = &input[0];
        let zoom = evaluation_context.get("zoom").map(|z| z.as_f64().expect("number") as f32);

        // Parse heatmap density
        let heatmap_density = evaluation_context.get("heatmapDensity").map(|h| h.as_f64().expect("number"));

        // Parse canonicalID
        let canonical = evaluation_context.get("canonicalID").map(|c| {
            let get = |k: &str| c[k].as_u64().expect("uint") as u32;
            CanonicalTileID::new(get("z") as u8, get("x"), get("y"))
        });

        // Parse availableImages
        let mut available_images = VecSet::new();
        if let Some(images) = evaluation_context.get("availableImages") {
            for image in images.as_array().expect("array") {
                available_images.insert(as_str(image));
            }
        }

        // Parse feature properties
        let mut feature = Feature::new(geo_types::Point::new(0.0, 0.0));
        let feature_object = &input[1];
        if let Some(properties) = feature_object.get("properties")
            && let Some(Value::Object(p)) = json_to_value(properties)
        {
            feature.properties = p;
        }

        if let Some(geometry) = feature_object.get("geometry") {
            feature.geometry = match geometry {
                serde_json::Value::Null => None,
                json => Some(
                    serde_json::from_value::<geojson::Geometry>(json.clone())
                        .map_err(geojson::Error::from)
                        .and_then(feature::geojson::geometry)
                        .map_err(|e| e.to_string())?,
                ),
            };
        }

        if let Some(id) = feature_object.get("id") {
            feature.id = convert_identifier(id);
        }

        data.inputs.push(Input { zoom, heatmap_density, canonical, available_images, feature });
    }
    Ok(())
}

/// The corpus root and the test directories (`parseArguments`).
#[must_use]
pub(crate) fn collect_test_paths(root_path: &Path, test_names: &[String], filter: Option<&str>) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.file_name().is_some_and(|f| f == "test.json") {
                out.push(path);
            }
        }
    }

    let mut paths: Vec<PathBuf> = test_names.iter().map(|n| root_path.join(n)).collect();
    if paths.is_empty() {
        paths.push(root_path.to_owned());
    }

    let mut test_paths = Vec::new();
    for path in paths {
        if !path.exists() {
            eprintln!("Provided test folder '{}' does not exist.", path.display());
            continue;
        }
        walk(&path, &mut test_paths);
    }
    if let Some(filter) = filter {
        test_paths.retain(|p| p.to_string_lossy().contains(filter));
    }
    test_paths
}

#[must_use]
pub(crate) fn parse_expression_ignores(ignores_path: &Path) -> Ignores {
    let Ok(document) = read_json(ignores_path) else {
        return Vec::new();
    };
    let mut ignores = Vec::new();
    for (id, reason) in document.as_object().into_iter().flatten() {
        // Keep only expression-test ignores
        if !id.starts_with("expression-tests") {
            continue;
        }
        ignores.push(Ignore { id: id.clone(), reason: reason.as_str().unwrap_or_default().to_owned() });
    }
    ignores
}

pub(crate) fn parse_test_data(path: &Path) -> Result<TestData, String> {
    let document = read_json(path).map_err(|e| format!("Cannot parse test '{}': {e}", path.display()))?;
    let mut data = TestData {
        inputs: Vec::new(),
        expected: TestResult::default(),
        result: TestResult::default(),
        recompiled: TestResult::default(),
        spec: None,
        document: serde_json::Value::Null,
    };

    // Check that mandatory test data members are present.
    if document.get("expression").is_none() || document.get("expected").is_none() {
        return Err(format!("Test fixture '{}' does not contain required data.", path.display()));
    }

    // Parse propertySpec
    if let Some(spec) = document.get("propertySpec") {
        parse_property_spec(spec, &mut data);
    }

    // Parse expected
    parse_expected(&document["expected"], &mut data);

    // Parse inputs
    if let Some(inputs) = document.get("inputs") {
        parse_inputs(inputs, &mut data).map_err(|e| format!("Cannot load test data from '{}': {e}", path.display()))?;
    }

    data.document = document;
    Ok(data)
}

/// Pretty-printed JSON, for diff output only.
#[must_use]
pub(crate) fn to_json(value: &Value) -> String {
    serde_json::to_string_pretty(&mbgl_to_json_for_display(value)).unwrap_or_default()
}

fn mbgl_to_json_for_display(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Uint(u) => serde_json::Value::from(*u),
        Value::Int(i) => serde_json::Value::from(*i),
        Value::Double(d) => serde_json::Number::from_f64(*d)
            .map_or_else(|| serde_json::Value::String(d.to_string()), serde_json::Value::Number),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Array(a) => serde_json::Value::Array(a.iter().map(mbgl_to_json_for_display).collect()),
        Value::Object(o) => {
            serde_json::Value::Object(o.iter().map(|(k, v)| (k.clone(), mbgl_to_json_for_display(v))).collect())
        }
    }
}

/// The JSON document the round trip re-parses. The C++ runner writes the
/// value with `writeJSON` (integral doubles through
/// `Int64(static_cast<int64_t>(d))`, NaN/Inf make the JSON invalid) and
/// parses it back with rapidjson.
#[must_use]
pub(crate) fn to_document(value: &Value) -> serde_json::Value {
    fn has_nan(value: &Value) -> bool {
        match value {
            Value::Double(d) => d.is_nan(),
            Value::Array(a) => a.iter().any(has_nan),
            Value::Object(o) => o.values().any(has_nan),
            _ => false,
        }
    }

    fn convert(value: &Value) -> serde_json::Value {
        match value {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Uint(u) => serde_json::Value::from(*u),
            Value::Int(i) => serde_json::Value::from(*i),
            Value::Double(d) => {
                if *d == d.floor() {
                    // static_cast<int64_t>(d) as x86-64 computes it.
                    let i = if *d >= -9_223_372_036_854_775_808.0 && *d < 9_223_372_036_854_775_808.0 {
                        *d as i64
                    } else {
                        i64::MIN
                    };
                    // A non-negative integer reads back as uint64.
                    if i >= 0 { serde_json::Value::from(i as u64) } else { serde_json::Value::from(i) }
                } else {
                    serde_json::Value::Number(serde_json::Number::from_f64(*d).unwrap())
                }
            }
            Value::String(s) => serde_json::Value::String(s.clone()),
            Value::Array(a) => serde_json::Value::Array(a.iter().map(convert).collect()),
            Value::Object(o) => serde_json::Value::Object(o.iter().map(|(k, v)| (k.clone(), convert(v))).collect()),
        }
    }

    if has_nan(value) {
        // The JSON text is invalid; the rapidjson document stays null.
        return serde_json::Value::Null;
    }
    convert(value)
}

#[must_use]
pub(crate) fn compiled_to_value(compiled: &Compiled) -> Value {
    let mut compiled_object = ValueObject::new();
    compiled_object
        .insert("result".to_owned(), Value::String(if compiled.success { "success" } else { "error" }.to_owned()));
    if compiled.success {
        compiled_object.insert("isFeatureConstant".to_owned(), Value::Bool(compiled.is_feature_constant));
        compiled_object.insert("isZoomConstant".to_owned(), Value::Bool(compiled.is_zoom_constant));
        compiled_object.insert("type".to_owned(), Value::String(compiled.serialized_type.clone()));
    } else {
        compiled_object.insert("errors".to_owned(), Value::Array(compiled.errors.clone()));
    }
    Value::Object(compiled_object)
}

/// Serializes an expression value to the JS format the tests expect; colors
/// and formatted values have their own forms.
pub(crate) fn expression_value_to_value(expr_value: &mln_expression::Value) -> Value {
    use mln_expression::value::to_mbgl_value;
    match expr_value {
        mln_expression::Value::Color(c) => Value::Array(vec![
            Value::Double(f64::from(c.r)),
            Value::Double(f64::from(c.g)),
            Value::Double(f64::from(c.b)),
            Value::Double(f64::from(c.a)),
        ]),
        mln_expression::Value::Formatted(formatted) => formatted.to_object(),
        mln_expression::Value::Array(values) => Value::Array(values.iter().map(to_mbgl_value).collect()),
        mln_expression::Value::Object(value_map) => {
            Value::Object(value_map.iter().map(|(k, v)| (k.clone(), to_mbgl_value(v))).collect())
        }
        v => to_mbgl_value(v),
    }
}

pub(crate) fn parse_expression(
    value: &serde_json::Value,
    spec: Option<&PropertySpec>,
    result: &mut TestResult,
) -> Option<Box<dyn Expression>> {
    let expected = spec.and_then(to_expression_type);
    let mut ctx = match &expected {
        Some(t) => ParsingContext::with_expected(t.clone()),
        None => ParsingContext::new(),
    };
    let convertible = Convertible::from(value);
    let parsed;
    if value.is_object()
        && let Some(expected) = &expected
    {
        let mut error = mln_conversion::Error::default();
        parsed = mln_style::conversion::function::convert_function_to_expression_for_type(
            expected,
            &convertible,
            &mut error,
            false, /*convert tokens*/
        );
        // TODO: should the error message be checked for function conversion?
    } else {
        parsed = ctx.parse_layer_property_expression(&convertible);
        if parsed.is_none() {
            for parsing_error in ctx.get_errors() {
                let mut error_map = ValueObject::new();
                error_map.insert("key".to_owned(), Value::String(parsing_error.key));
                error_map.insert("error".to_owned(), Value::String(parsing_error.message));
                result.compiled.errors.push(Value::Object(error_map));
            }
        }
    }

    result.expression = json_to_value(value);
    result.compiled.success = parsed.is_some();
    if let Some(p) = &parsed {
        result.compiled.is_feature_constant = is_feature_constant(&**p);
        result.compiled.is_zoom_constant = is_zoom_constant(&**p);
        result.compiled.serialized_type = type_::to_string(p.get_type());
        result.serialized = Some(p.serialize());
    }
    parsed
}
