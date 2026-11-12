// Emits the generated half of //difftest/style_codegen from the same spec:
// a C++ file with the per-property calls (`XLayer::getDefaultYyy()`, layer
// construction per type) and a Rust module with the matching calls plus the
// case list: for every layer type and every paint/layout property, sample
// JSON values chosen from the property's type (constants, zoom and
// data expressions, legacy functions, transitions, invalid values).

use serde_json::Value;

use crate::emit_layer::is_source_less;
use crate::spec::{LayerSpec, Property};
use crate::types::{EvaluatedType, camelize, evaluated_type, is_color_ramp, is_data_driven, snake};
use crate::writer::{Writer, layer_file_name};

/// The enumerator names of an enum property (the spec's `values` keys).
fn enum_values(p: &Property) -> Vec<String> {
    match p.spec.get("values") {
        Some(Value::Object(values)) => values.keys().cloned().collect(),
        Some(Value::Array(values)) => values.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        _ => panic!("{}: enum without values", p.name),
    }
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// Valid constant JSON values for a property of type `t`.
fn constants(p: &Property, t: &EvaluatedType) -> Vec<String> {
    match t {
        EvaluatedType::Bool => vec!["true".into(), "false".into()],
        EvaluatedType::Float => vec!["1.5".into(), "0".into(), "-2.25".into(), "300".into()],
        EvaluatedType::Double => vec!["1.5".into(), "-0.5".into()],
        EvaluatedType::Rotation => vec!["45.5".into(), "-90".into(), "400".into()],
        EvaluatedType::String => vec![json_string("abc")],
        EvaluatedType::Color => vec![
            json_string("#ff8000"),
            json_string("rgba(10, 20, 30, 0.5)"),
            json_string("hsl(120, 50%, 50%)"),
            json_string("transparent"),
        ],
        EvaluatedType::Padding => vec!["3".into(), "[1, 2]".into(), "[1, 2, 3]".into(), "[1, 2, 3, 4]".into()],
        EvaluatedType::Image => {
            let mut v = vec![json_string("marker-15"), json_string("")];
            if p.name == "icon-image" {
                v.push(json_string("{maki}-15"));
            }
            v
        }
        EvaluatedType::Formatted => {
            let mut v = vec![json_string("Hello"), json_string("")];
            if p.name == "text-field" {
                v.push(json_string("{name} ({ref})"));
            }
            v
        }
        EvaluatedType::VariableAnchorOffsetCollection => {
            vec![r#"["top", [0, 1], "bottom", [1.5, -2]]"#.into(), r#"["center", [0, 0]]"#.into()]
        }
        EvaluatedType::Position => vec!["[1.5, 90, 45]".into(), "[0, 0, 0]".into()],
        EvaluatedType::Enum(_) => enum_values(p).iter().map(|v| json_string(v)).collect(),
        EvaluatedType::Array(element, n) => {
            let base: Vec<&str> = match **element {
                EvaluatedType::Float | EvaluatedType::Double => vec!["1.5", "-2", "3.25", "4"],
                _ => panic!("{}: array of {:?}", p.name, element),
            };
            vec![format!("[{}]", base[..*n].join(", ")), format!("[{}]", vec!["0"; *n].join(", "))]
        }
        EvaluatedType::Vector(element) => match &**element {
            EvaluatedType::Float => vec!["[1, 2, 3]".into(), "5".into(), "[]".into()],
            EvaluatedType::String => vec![r#"["Open Sans Bold", "Arial Unicode MS Bold"]"#.into(), "[]".into()],
            EvaluatedType::Color => vec![r##"["#000000", "#ffffff"]"##.into(), json_string("red")],
            EvaluatedType::Enum(_) => {
                let values = enum_values(p);
                vec![
                    format!("[{}]", values.iter().map(|v| json_string(v)).collect::<Vec<_>>().join(", ")),
                    format!("[{}]", json_string(&values[0])),
                    "[]".into(),
                ]
            }
            other => panic!("{}: vector of {:?}", p.name, other),
        },
    }
}

/// A constant written as an expression literal (arrays need `literal`).
fn literal(json: &str) -> String {
    if json.starts_with('[') { format!(r#"["literal", {json}]"#) } else { json.to_owned() }
}

/// Values of the wrong type.
fn invalid(t: &EvaluatedType) -> Vec<String> {
    let mut v = vec![r#"{"foo": 1}"#.to_owned(), "null".to_owned()];
    match t {
        EvaluatedType::Bool => v.push("1".into()),
        EvaluatedType::Float | EvaluatedType::Rotation | EvaluatedType::Double => v.push(json_string("abc")),
        EvaluatedType::Color => v.extend([json_string("not-a-color"), "5".into()]),
        EvaluatedType::Enum(_) => v.extend([json_string("bogus"), "5".into()]),
        EvaluatedType::Padding => v.extend(["[]".into(), "[1, 2, 3, 4, 5]".into(), json_string("abc")]),
        EvaluatedType::Array(_, n) => v.extend([format!("[{}]", vec!["1"; n + 1].join(", ")), json_string("abc")]),
        EvaluatedType::VariableAnchorOffsetCollection => {
            v.extend([r#"["top"]"#.into(), r#"["top", [1]]"#.into(), r#"["nowhere", [0, 0]]"#.into()]);
        }
        _ => v.push("5".into()),
    }
    v
}

fn interpolatable(t: &EvaluatedType) -> bool {
    match t {
        EvaluatedType::Float | EvaluatedType::Color | EvaluatedType::Padding | EvaluatedType::Rotation => true,
        EvaluatedType::Array(element, _) => matches!(**element, EvaluatedType::Float | EvaluatedType::Double),
        _ => false,
    }
}

fn is_interpolated(p: &Property) -> bool {
    p.spec.get("expression").and_then(|e| e.get("interpolated")).and_then(Value::as_bool).unwrap_or(false)
}

/// Sample JSON values for one property.
pub fn samples(p: &Property) -> Vec<String> {
    if is_color_ramp(p) {
        let parameter = p
            .spec
            .get("expression")
            .and_then(|e| e.get("parameters"))
            .and_then(Value::as_array)
            .and_then(|params| params.iter().filter_map(Value::as_str).find(|s| *s != "zoom"))
            .unwrap_or_else(|| panic!("{}: color ramp without a parameter", p.name))
            .to_owned();
        return vec![
            format!(r#"["interpolate", ["linear"], ["{parameter}"], 0, "rgba(0, 0, 255, 0)", 1, "red"]"#),
            format!(r#"["step", ["{parameter}"], "blue", 0.5, "red"]"#),
            r#"["interpolate", ["linear"], ["zoom"], 0, "blue", 10, "red"]"#.into(),
            r#"["to-color", ["get", "c"]]"#.into(),
            json_string("red"),
            "null".into(),
        ];
    }

    let t = evaluated_type(p);
    let c = constants(p, &t);
    let a = &c[0];
    let b = c.get(1).unwrap_or(&c[0]);
    let mut v = c.clone();
    if interpolatable(&t) && is_interpolated(p) {
        v.push(format!(r#"["interpolate", ["linear"], ["zoom"], 0, {}, 10, {}]"#, literal(a), literal(b)));
        v.push(format!(r#"["interpolate", ["exponential", 2], ["zoom"], 5, {}, 15, {}]"#, literal(a), literal(b)));
    }
    v.push(format!(r#"["step", ["zoom"], {}, 10, {}]"#, literal(a), literal(b)));
    // Legacy functions.
    v.push(format!(r#"{{"stops": [[0, {a}], [10, {b}]]}}"#));
    v.push(format!(r#"{{"type": "interval", "stops": [[0, {a}], [10, {b}]]}}"#));
    if is_data_driven(p) {
        v.push(format!(r#"{{"property": "k", "type": "categorical", "stops": [["x", {a}], ["y", {b}]]}}"#));
    }
    // A data expression: kept for data-driven properties, an error for
    // the others.
    v.push(format!(r#"["case", ["has", "k"], {}, {}]"#, literal(a), literal(b)));
    // A global-state expression (runtime-dependent).
    v.push(format!(r#"["case", ["global-state", "flag"], {}, {}]"#, literal(a), literal(b)));
    // A constant expression (folded to a constant). Not for images: the
    // `image` coercion is not folded, and an expression that is feature-,
    // zoom- and runtime-constant but not a literal hits `assert(false)` in
    // C++ conversion/property_value.cpp (a precondition, so no such input).
    if t != EvaluatedType::Image {
        v.push(format!(r#"["case", true, {}, {}]"#, literal(a), literal(b)));
    }
    v.extend(invalid(&t));
    v
}

fn transition_samples() -> Vec<String> {
    vec![r#"{"duration": 300, "delay": 50}"#.into(), r#"{"duration": 0}"#.into(), "{}".into(), "5".into()]
}

/// One whole layer JSON per type, every property at its first sample, for
/// `convert<std::unique_ptr<Layer>>`.
fn layer_json(layer: &LayerSpec, paint: &[&Property]) -> String {
    let mut members = vec![r#""id": "layer""#.to_owned(), format!(r#""type": "{}""#, layer.type_)];
    if !is_source_less(&layer.type_) {
        members.push(r#""source": "source""#.into());
    }
    members.push(r#""source-layer": "source-layer""#.into());
    members.push(r#""minzoom": 2"#.into());
    members.push(r#""maxzoom": 18.5"#.into());
    members.push(r#""filter": ["==", ["get", "k"], 1]"#.into());
    let mut layout = vec![r#""visibility": "none""#.to_owned()];
    for p in &layer.layout_properties {
        layout.push(format!("{}: {}", json_string(&p.name), samples(p)[0]));
    }
    members.push(format!(r#""layout": {{{}}}"#, layout.join(", ")));
    let mut paint_members = Vec::new();
    for p in paint {
        paint_members.push(format!("{}: {}", json_string(&p.name), samples(p)[0]));
        paint_members.push(format!(r#"{}: {{"duration": 10}}"#, json_string(&format!("{}-transition", p.name))));
    }
    members.push(format!(r#""paint": {{{}}}"#, paint_members.join(", ")));
    format!("{{{}}}", members.join(", "))
}

fn rust_str(s: &str) -> String {
    let mut hashes = String::from("#");
    while s.contains(&format!("\"{hashes}")) {
        hashes.push('#');
    }
    format!("r{hashes}\"{s}\"{hashes}")
}

/// The `pe_*` helper (`style_codegen_difftest.rs`) for a property's
/// `PossiblyEvaluatedType`.
fn possibly_evaluated_kind(p: &Property, layout: bool) -> &'static str {
    if layout {
        return if is_data_driven(p) { "pe_pepv" } else { "pe_plain" };
    }
    match p.property_type() {
        Some("data-driven") => "pe_pepv",
        Some("cross-faded-data-driven") => "pe_pefaded",
        Some("cross-faded") => "pe_faded",
        _ => "pe_plain",
    }
}

/// `(generated.cpp text, cases.rs text)`.
pub fn generate(spec_text: &str) -> Result<(String, String), String> {
    let spec = crate::spec::load(spec_text)?;
    let layers = crate::spec::layers(&spec)?;
    let light = crate::spec::light_properties(&spec)?;

    // --- C++ ---
    let mut cpp = Writer::new();
    cpp.line("// This file is generated by //tools/style-codegen (src/difftest.rs) from the");
    cpp.line("// style spec: the per-property C++ calls of //difftest/style_codegen.");
    cpp.blank();
    cpp.line("#include \"difftest/style_codegen/generated.hpp\"");
    cpp.blank();
    for layer in &layers {
        let file = layer_file_name(&layer.type_);
        cpp.line(&format!("#include <mln/style/layers/{file}_layer.hpp>"));
        cpp.line(&format!("#include <mln/style/layers/{file}_layer_impl.hpp>"));
        cpp.line(&format!("#include <mln/style/layers/{file}_layer_properties.hpp>"));
    }
    cpp.line("#include <mln/map/zoom_history.hpp>");
    cpp.line("#include <mln/renderer/property_evaluation_parameters.hpp>");
    cpp.line("#include <mln/style/light.hpp>");
    cpp.line("#include <mln/style/conversion_impl.hpp>");
    cpp.blank();
    cpp.line("namespace codegen_difftest {");
    cpp.blank();
    cpp.line("using namespace mln;");
    cpp.line("using namespace mln::style;");
    cpp.line("using namespace mln::style::conversion;");
    cpp.blank();
    cpp.line("std::unique_ptr<Layer> makeLayer(const std::string& type) {");
    for layer in &layers {
        let class = format!("{}Layer", camelize(&layer.type_));
        if is_source_less(&layer.type_) {
            cpp.line(&format!("    if (type == \"{}\") return std::make_unique<{class}>(\"layer\");", layer.type_));
        } else {
            cpp.line(&format!(
                "    if (type == \"{}\") return std::make_unique<{class}>(\"layer\", \"source\");",
                layer.type_
            ));
        }
    }
    cpp.line("    return nullptr;");
    cpp.line("}");
    cpp.blank();
    cpp.line("std::optional<StyleProperty> layerDefault(const std::string& type, const std::string& name) {");
    for layer in &layers {
        let class = format!("{}Layer", camelize(&layer.type_));
        cpp.line(&format!("    if (type == \"{}\") {{", layer.type_));
        for p in
            layer.layout_properties.iter().chain(layer.paint_properties.iter().filter(|p| p.name != "line-floor-width"))
        {
            cpp.line(&format!(
                "        if (name == \"{}\") return makeStyleProperty({class}::getDefault{}());",
                p.name,
                camelize(&p.name)
            ));
        }
        cpp.line("        return std::nullopt;");
        cpp.line("    }");
    }
    cpp.line("    return std::nullopt;");
    cpp.line("}");
    cpp.blank();
    cpp.line("std::optional<Value> lightDefault(const std::string& name) {");
    for p in &light {
        cpp.line(&format!(
            "    if (name == \"{}\") return makeValue(Light::getDefault{}());",
            p.name,
            camelize(&p.name)
        ));
    }
    cpp.line("    return std::nullopt;");
    cpp.line("}");
    cpp.blank();
    cpp.line("std::string evaluateLayer(const Layer& layer, float zoom) {");
    cpp.line("    const TimePoint now{Duration(std::chrono::seconds(1))};");
    cpp.line("    ZoomHistory zoomHistory;");
    cpp.line("    zoomHistory.update(zoom, now);");
    cpp.line("    const PropertyEvaluationParameters parameters{zoomHistory, now, Duration::zero()};");
    cpp.line("    std::string out;");
    for layer in &layers {
        let class = format!("{}Layer", camelize(&layer.type_));
        cpp.line(&format!("    if (layer.getTypeInfo() == {class}::Impl::staticTypeInfo()) {{"));
        cpp.line(&format!("        const auto& impl = static_cast<const {class}::Impl&>(*layer.baseImpl);"));
        cpp.line("        const auto paint = impl.paint.untransitioned().evaluate(parameters);");
        for p in &layer.paint_properties {
            cpp.line(&format!("        out += \"{}=\" + pe(paint.get<{}>()) + \";\";", p.name, camelize(&p.name)));
        }
        if !layer.layout_properties.is_empty() {
            cpp.line("        const auto layout = impl.layout.evaluate(parameters);");
            for p in &layer.layout_properties {
                cpp.line(&format!("        out += \"{}=\" + pe(layout.get<{}>()) + \";\";", p.name, camelize(&p.name)));
            }
        }
        cpp.line("        return out;");
        cpp.line("    }");
    }
    cpp.line("    return \"?\";");
    cpp.line("}");
    cpp.blank();
    cpp.line("} // namespace codegen_difftest");

    // --- Rust ---
    let mut rs = Writer::new();
    rs.line("// This file is generated by //tools/style-codegen (src/difftest.rs) from the");
    rs.line("// style spec: the per-property Rust calls and the cases of");
    rs.line("// //difftest/style_codegen.");
    rs.blank();
    rs.line("use feature::Value;");
    rs.line("use mln_style::conversion_impl::{make_color_ramp_style_property, make_style_property, make_value};");
    rs.line("use mln_style::layer::Layer;");
    rs.line("use mln_style::layers::{");
    for layer in &layers {
        rs.line(&format!("    {}Layer,", camelize(&layer.type_)));
    }
    rs.line("};");
    rs.line("use mln_style::light::Light;");
    rs.line("use mln_style::style_property::StyleProperty;");
    rs.blank();
    rs.line("pub fn make_layer(type_: &str) -> Option<Box<dyn Layer>> {");
    rs.line("    match type_ {");
    for layer in &layers {
        let class = format!("{}Layer", camelize(&layer.type_));
        if is_source_less(&layer.type_) {
            rs.line(&format!("        \"{}\" => Some(Box::new({class}::new(\"layer\"))),", layer.type_));
        } else {
            rs.line(&format!("        \"{}\" => Some(Box::new({class}::new(\"layer\", \"source\"))),", layer.type_));
        }
    }
    rs.line("        _ => None,");
    rs.line("    }");
    rs.line("}");
    rs.blank();
    rs.line("pub fn layer_default(type_: &str, name: &str) -> Option<StyleProperty> {");
    rs.line("    match (type_, name) {");
    let mut properties = Vec::new();
    let mut cases = Vec::new();
    let mut layer_cases = Vec::new();
    for layer in &layers {
        let class = format!("{}Layer", camelize(&layer.type_));
        let paint: Vec<&Property> = layer.paint_properties.iter().filter(|p| p.name != "line-floor-width").collect();
        let all = layer.layout_properties.iter().map(|p| (p, false)).chain(paint.iter().map(|p| (*p, true)));
        for (p, is_paint) in all {
            let make = if is_color_ramp(p) { "make_color_ramp_style_property" } else { "make_style_property" };
            rs.line(&format!(
                "        (\"{}\", \"{}\") => Some({make}(&{class}::get_default_{}())),",
                layer.type_,
                p.name,
                snake(&p.name)
            ));
            properties.push(format!("(\"{}\", \"{}\", {is_paint})", layer.type_, p.name));
            for sample in samples(p) {
                cases.push(format!("(\"{}\", \"{}\", {is_paint}, {})", layer.type_, p.name, rust_str(&sample)));
            }
            if is_paint {
                for sample in transition_samples() {
                    cases.push(format!(
                        "(\"{}\", \"{}-transition\", true, {})",
                        layer.type_,
                        p.name,
                        rust_str(&sample)
                    ));
                }
            }
        }
        // A name the layer does not have, and the base properties.
        for (name, sample) in [
            ("no-such-property", "1"),
            ("visibility", r#""none""#),
            ("visibility", r#""bogus""#),
            ("minzoom", "3.5"),
            ("maxzoom", r#""x""#),
            ("source-layer", r#""roads""#),
            ("filter", r#"["==", ["get", "k"], 1]"#),
        ] {
            cases.push(format!("(\"{}\", \"{name}\", false, {})", layer.type_, rust_str(sample)));
        }
        layer_cases.push(format!("(\"{}\", {})", layer.type_, rust_str(&layer_json(layer, &paint))));
    }
    rs.line("        _ => None,");
    rs.line("    }");
    rs.line("}");
    rs.blank();
    rs.line("pub fn light_default(name: &str) -> Option<Value> {");
    rs.line("    match name {");
    for p in &light {
        rs.line(&format!("        \"{}\" => Some(make_value(&Light::get_default_{}())),", p.name, snake(&p.name)));
    }
    rs.line("        _ => None,");
    rs.line("    }");
    rs.line("}");
    rs.blank();
    rs.line("/// `codegen_difftest::evaluateLayer` (generated.cpp).");
    rs.line("pub fn evaluate_layer(layer: &dyn Layer, zoom: f32) -> String {");
    rs.line("    let parameters = super::evaluation_parameters(zoom);");
    rs.line("    let base_impl = layer.base_impl();");
    rs.line("    let mut out = String::new();");
    rs.line("    match layer.get_type_info().type_ {");
    for layer in &layers {
        let file = layer_file_name(&layer.type_);
        rs.line(&format!("        \"{}\" => {{", layer.type_));
        rs.line(&format!(
            "            let impl_ = base_impl.downcast_ref::<mln_style::layers::{file}_layer::Impl>().unwrap();"
        ));
        rs.line("            let paint = impl_.paint.untransitioned().evaluate(&parameters);");
        for p in &layer.paint_properties {
            rs.line(&format!(
                "            out.push_str(&format!(\"{}={{}};\", super::{}(&paint.{})));",
                p.name,
                possibly_evaluated_kind(p, false),
                snake(&p.name)
            ));
        }
        if !layer.layout_properties.is_empty() {
            rs.line("            let layout = impl_.layout.evaluate(&parameters);");
            for p in &layer.layout_properties {
                rs.line(&format!(
                    "            out.push_str(&format!(\"{}={{}};\", super::{}(&layout.{})));",
                    p.name,
                    possibly_evaluated_kind(p, true),
                    snake(&p.name)
                ));
            }
        }
        rs.line("        }");
    }
    rs.line("        _ => return \"?\".to_owned(),");
    rs.line("    }");
    rs.line("    out");
    rs.line("}");
    rs.blank();
    rs.line("/// `(layer type, property, is paint)` for every generated property.");
    rs.line("pub const PROPERTIES: &[(&str, &str, bool)] = &[");
    for p in &properties {
        rs.line(&format!("    {p},"));
    }
    rs.line("];");
    rs.blank();
    rs.line("pub const LIGHT_PROPERTIES: &[&str] = &[");
    for p in &light {
        rs.line(&format!("    \"{}\",", p.name));
    }
    rs.line("];");
    rs.blank();
    rs.line("/// `(layer type, property, is paint, JSON value)`.");
    rs.line("pub const CASES: &[(&str, &str, bool, &str)] = &[");
    for c in &cases {
        rs.line(&format!("    {c},"));
    }
    rs.line("];");
    rs.blank();
    rs.line("/// `(property, JSON value)` for `Light::setProperty`.");
    rs.line("pub const LIGHT_CASES: &[(&str, &str)] = &[");
    for p in &light {
        for sample in samples(p) {
            rs.line(&format!("    (\"{}\", {}),", p.name, rust_str(&sample)));
        }
        for sample in transition_samples() {
            rs.line(&format!("    (\"{}-transition\", {}),", p.name, rust_str(&sample)));
        }
    }
    rs.line("    (\"no-such-property\", \"1\"),");
    rs.line("];");
    rs.blank();
    rs.line("/// `(layer type, layer JSON)` for `convert<std::unique_ptr<Layer>>`.");
    rs.line("pub const LAYER_CASES: &[(&str, &str)] = &[");
    for c in &layer_cases {
        rs.line(&format!("    {c},"));
    }
    rs.line("];");

    Ok((cpp.finish(), rs.finish()))
}
