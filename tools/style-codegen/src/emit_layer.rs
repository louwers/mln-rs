use crate::spec::{LayerSpec, Property};
use crate::types::{camelize, evaluated_type, is_color_ramp, is_data_driven, property_value_type, snake};
use crate::writer::{MAX_WIDTH, Writer, if_condition, imports_for, layer_file_name};

/// The layer type's capabilities: (Source, `Pass3D`, Layout, `FadingTiles`,
/// `CrossTileIndex`, `TileKind`).
fn layer_capabilities(type_: &str) -> [&'static str; 6] {
    const N: &str = "NotRequired";
    const R: &str = "Required";
    match type_ {
        "background" => [N, N, N, N, N, N],
        "fill" | "line" | "circle" => [R, N, R, N, N, "Geometry"],
        "fill-extrusion" => [R, R, R, N, N, "Geometry"],
        "hillshade" => [R, R, N, N, N, "RasterDEM"],
        "color-relief" => [R, N, N, N, N, "RasterDEM"],
        "symbol" => [R, N, R, R, R, "Geometry"],
        "heatmap" => [R, R, N, N, N, "Geometry"],
        "raster" => [R, N, N, N, N, "Raster"],
        other => panic!("layer.cpp.ejs has no capabilities for layer type {other}"),
    }
}

/// Whether the layer type's constructor takes no source ID.
#[must_use]
pub(crate) fn is_source_less(type_: &str) -> bool {
    type_ == "background"
}

/// The conversion call that `set_property_internal` makes for a property
/// (also the grouping key, as in the template).
#[must_use]
pub(crate) fn conversion(property: &Property) -> String {
    let data_driven = is_data_driven(property);
    let convert_tokens = property.name == "icon-image" || property.name == "text-field";
    if is_color_ramp(property) {
        format!("convert_color_ramp_property_value(value, &mut error, {data_driven}, {convert_tokens})")
    } else {
        format!(
            "convert_property_value::<{}>(value, &mut error, {data_driven}, {convert_tokens})",
            evaluated_type(property).rust()
        )
    }
}

/// Groups properties by conversion, in order of first appearance (JS object
/// key order of the template's `conversions`).
pub(crate) fn conversion_groups<'a>(
    properties: impl Iterator<Item = &'a Property>,
) -> Vec<(String, Vec<&'a Property>)> {
    let mut groups: Vec<(String, Vec<&'a Property>)> = Vec::new();
    for p in properties {
        let key = conversion(p);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(p),
            None => groups.push((key, vec![p])),
        }
    }
    groups
}

/// Writes `let Some(typed_value) = <key> else { return Some(error); };`,
/// wrapped as rustfmt does past the line width.
pub(crate) fn let_else(w: &mut Writer, indent: &str, key: &str) {
    let one_line = format!("{indent}let Some(typed_value) = {key} else {{");
    if one_line.len() <= MAX_WIDTH {
        w.line(&one_line);
    } else {
        w.line(&format!("{indent}let Some(typed_value) ="));
        w.line(&format!("{indent}    {key}"));
        w.line(&format!("{indent}else {{"));
    }
    w.line(&format!("{indent}    return Some(error);"));
    w.line(&format!("{indent}}};"));
}

fn make_style_property(property: &Property) -> &'static str {
    if is_color_ramp(property) { "make_color_ramp_style_property" } else { "make_style_property" }
}

fn emit_accessors(w: &mut Writer, layer: &LayerSpec, property: &Property, paint: bool) {
    let name = camelize(&property.name);
    let snake_name = snake(&property.name);
    let pv = property_value_type(property);
    let member = if paint { "paint" } else { "layout" };

    // The renderer falls back to these two color ramps' defaults when the
    // style leaves them undefined; no other property's default has a caller.
    if property.name == "heatmap-color" {
        let raw = crate::json::js_stringify(property.default().expect("heatmap-color default"));
        assert!(!raw.contains("\"#"), "raw string delimiter in {raw}");
        w.line("    #[must_use]");
        w.line(&format!("    pub fn get_default_{snake_name}() -> {pv} {{"));
        w.line("        let mut error = Error::default();");
        w.line("        let raw_value: serde_json::Value = serde_json::from_str(");
        w.line(&format!("            r#\"{raw}\"#,"));
        w.line("        )");
        w.line("        .unwrap();");
        w.line("        convert_color_ramp_property_value(&Convertible::from(&raw_value), &mut error, false, false)");
        w.line("            .expect(\"the heatmap-color default converts\")");
        w.line("    }");
        w.blank();
    } else if property.name == "color-relief-color" {
        w.line("    #[must_use]");
        w.line(&format!("    pub fn get_default_{snake_name}() -> {pv} {{"));
        w.line("        ColorRampPropertyValue::default()");
        w.line("    }");
        w.blank();
    }

    if paint {
        w.line("    #[must_use]");
        w.line(&format!("    pub fn get_{snake_name}(&self) -> &{pv} {{"));
        w.line(&format!("        &self.impl_().paint.get::<{name}>().value"));
        w.line("    }");
    } else {
        w.line("    #[must_use]");
        w.line(&format!("    pub fn get_{snake_name}(&self) -> &{pv} {{"));
        w.line(&format!("        self.impl_().layout.get::<{name}>()"));
        w.line("    }");
    }
    w.blank();

    w.line(&format!("    pub fn set_{snake_name}(&mut self, value: &{pv}) {{"));
    w.line(&format!("        if *value == *self.get_{snake_name}() {{"));
    w.line("            return;");
    w.line("        }");
    w.line("        let mut impl_ = self.mutable_impl();");
    if paint {
        w.line(&format!("        impl_.paint.get_mut::<{name}>().value = value.clone();"));
        if property.name == "line-width" {
            w.line("        impl_.paint.get_mut::<LineFloorWidth>().value = value.clone();");
        }
    } else {
        w.line(&format!("        *impl_.{member}.get_mut::<{name}>() = value.clone();"));
    }
    w.line("        self.set_impl(impl_);");
    w.line("    }");
    w.blank();

    if paint {
        w.line(&format!("    pub fn set_{snake_name}_transition(&mut self, options: &TransitionOptions) {{"));
        w.line("        let mut impl_ = self.mutable_impl();");
        w.line(&format!("        impl_.paint.get_mut::<{name}>().options = *options;"));
        w.line("        self.set_impl(impl_);");
        w.line("    }");
        w.blank();
        w.line("    #[must_use]");
        w.line(&format!("    pub fn get_{snake_name}_transition(&self) -> TransitionOptions {{"));
        w.line(&format!("        self.impl_().paint.get::<{name}>().options"));
        w.line("    }");
        w.blank();
    }
    let _ = layer;
}

/// The layer's `Property` variant for a property: the property's name,
/// camelized, without the layer type's prefix when every property of the
/// layer has it (`line-color` -> `Color`).
fn property_variant(layer: &LayerSpec, property: &Property) -> String {
    let prefix = format!("{}-", layer.type_);
    let mut properties = layer.paint_properties.iter().chain(&layer.layout_properties);
    let name = match property.name.strip_prefix(&prefix) {
        Some(stripped) if properties.all(|p| p.name.starts_with(&prefix)) => stripped,
        _ => &property.name,
    };
    camelize(name)
}

#[must_use]
pub(crate) fn emit(layer: &LayerSpec) -> String {
    let t = camelize(&layer.type_);
    let file = layer_file_name(&layer.type_);
    let layer_name = format!("{t}Layer");
    // "Remove our fake property for the external interface."
    let paint: Vec<&Property> = layer.paint_properties.iter().filter(|p| p.name != "line-floor-width").collect();
    let layout: Vec<&Property> = layer.layout_properties.iter().collect();
    let mut w = Writer::new();

    w.line("// This file is generated by //tools/style-codegen from the style spec");
    w.line("// (tools/style-codegen/v8.json). Do not edit.");
    w.blank();

    // Imports.
    let mut imports = imports_for(layer.layout_properties.iter().chain(&layer.paint_properties), true);
    let any_color_ramp = paint.iter().any(|p| is_color_ramp(p));
    let any_plain = paint.iter().chain(&layout).any(|p| !is_color_ramp(p));
    let any_convert_property_value =
        paint.iter().chain(&layout).any(|p| conversion(p).starts_with("convert_property_value::"));
    let mut conversion_items = vec!["Converter"];
    if any_color_ramp {
        conversion_items.push("convert_color_ramp_property_value");
    }
    if any_convert_property_value {
        conversion_items.push("convert_property_value");
    }
    imports.push(format!("crate::conversion::{{{}}}", conversion_items.join(", ")));
    let mut conversion_impl_items = vec!["make_transition_style_property"];
    if any_plain {
        conversion_impl_items.push("make_style_property");
    }
    if any_color_ramp {
        conversion_impl_items.push("make_color_ramp_style_property");
    }
    imports.push(format!("crate::conversion_impl::{{{}}}", conversion_impl_items.join(", ")));
    imports.push(
        "crate::layer::{self, ImplBase, Layer, LayerTypeInfo, layer_type_info, serialize_base, serialize_property}"
            .into(),
    );
    let mut tags: Vec<String> =
        layer.paint_properties.iter().chain(&layer.layout_properties).map(|p| camelize(&p.name)).collect();
    tags.push(format!("{t}PaintProperties"));
    imports.push(format!("crate::layers::{file}_layer_properties::{{{}}}", tags.join(", ")));
    if any_plain {
        imports.push("crate::property_value::PropertyValue".into());
    }
    imports.push("crate::style_property::{Kind, StyleProperty}".into());
    imports.push("crate::transition_options::TransitionOptions".into());
    imports.push("feature::Value".into());
    imports.push("mln_conversion::{Convertible, Error}".into());
    imports.push("std::sync::Arc".into());
    w.imports(imports);
    w.blank();
    w.line(&format!("pub use crate::layers::{file}_layer_impl::Impl;"));
    w.blank();

    // Type info.
    let caps = layer_capabilities(&layer.type_);
    w.line("static TYPE_INFO: LayerTypeInfo = LayerTypeInfo {");
    w.line(&format!("    type_: \"{}\",", layer.type_));
    let members = [
        ("source", "Source"),
        ("pass3d", "Pass3D"),
        ("layout", "Layout"),
        ("fading_tiles", "FadingTiles"),
        ("cross_tile_index", "CrossTileIndex"),
        ("tile_kind", "TileKind"),
    ];
    for ((member, enum_name), value) in members.iter().zip(caps) {
        w.line(&format!("    {member}: layer_type_info::{enum_name}::{value},"));
    }
    w.line("};");
    w.blank();
    w.line("impl Impl {");
    w.line("    #[must_use]");
    w.line("    pub fn static_type_info() -> &'static LayerTypeInfo {");
    w.line("        &TYPE_INFO");
    w.line("    }");
    w.line("}");
    w.blank();

    // The layer.
    w.line("#[derive(Debug)]");
    w.line(&format!("pub struct {layer_name} {{"));
    w.line("    base_impl: Arc<Impl>,");
    w.line("}");
    w.blank();
    w.line(&format!("impl {layer_name} {{"));
    if is_source_less(&layer.type_) {
        w.line("    #[must_use]");
        w.line("    pub fn new(layer_id: &str) -> Self {");
        w.line(&format!("        {layer_name} {{"));
        w.line("            base_impl: Arc::new(Impl::new(layer_id.to_owned(), String::new())),");
        w.line("        }");
        w.line("    }");
    } else {
        w.line("    #[must_use]");
        w.line("    pub fn new(layer_id: &str, source_id: &str) -> Self {");
        w.line(&format!("        {layer_name} {{"));
        w.line("            base_impl: Arc::new(Impl::new(layer_id.to_owned(), source_id.to_owned())),");
        w.line("        }");
        w.line("    }");
    }
    w.blank();
    w.line("    #[must_use]");
    w.line("    pub fn from_impl(impl_: Arc<Impl>) -> Self {");
    w.line(&format!("        {layer_name} {{ base_impl: impl_ }}"));
    w.line("    }");
    w.blank();
    w.line("    #[must_use]");
    w.line("    pub fn impl_(&self) -> &Impl {");
    w.line("        &self.base_impl");
    w.line("    }");
    w.blank();
    w.line("    /// A copy of the impl, to change and store back.");
    w.line("    #[must_use]");
    w.line("    pub fn mutable_impl(&self) -> Impl {");
    w.line("        self.impl_().clone()");
    w.line("    }");
    w.blank();
    w.line("    fn set_impl(&mut self, impl_: Impl) {");
    w.line("        self.base_impl = Arc::new(impl_);");
    w.line("    }");
    w.blank();
    if !layout.is_empty() {
        w.line("    // Layout properties");
        w.blank();
        for p in &layout {
            emit_accessors(&mut w, layer, p, false);
        }
    }
    w.line("    // Paint properties");
    w.blank();
    for p in &paint {
        emit_accessors(&mut w, layer, p, true);
    }
    w.finish_block();
    w.line("}");
    w.blank();

    // Property table.
    w.line(&format!("const K_PAINT_PROPERTY_COUNT: u8 = {};", paint.len() * 2));
    w.blank();
    w.line("#[derive(Clone, Copy, Debug, PartialEq, Eq)]");
    w.line("#[repr(u8)]");
    w.line("enum Property {");
    for p in &paint {
        w.line(&format!("    {},", property_variant(layer, p)));
    }
    for p in &paint {
        w.line(&format!("    {}Transition,", property_variant(layer, p)));
    }
    for (i, p) in layout.iter().enumerate() {
        if i == 0 {
            w.line(&format!("    {} = K_PAINT_PROPERTY_COUNT,", property_variant(layer, p)));
        } else {
            w.line(&format!("    {},", property_variant(layer, p)));
        }
    }
    w.line("}");
    w.blank();
    w.line("/// The style-spec name of each property.");
    w.line("const LAYER_PROPERTIES: &[(&str, Property)] = &[");
    for p in &paint {
        w.line(&format!("    (\"{}\", Property::{}),", p.name, property_variant(layer, p)));
    }
    for p in &paint {
        w.line(&format!("    (\"{}-transition\", Property::{}Transition),", p.name, property_variant(layer, p)));
    }
    for p in &layout {
        w.line(&format!("    (\"{}\", Property::{}),", p.name, property_variant(layer, p)));
    }
    w.line("];");
    w.blank();
    w.line("fn find_property(name: &str) -> Option<Property> {");
    w.line("    LAYER_PROPERTIES.iter().find(|(n, _)| *n == name).map(|(_, p)| *p)");
    w.line("}");
    w.blank();
    w.line(&format!("fn get_layer_property(layer: &{layer_name}, property: Property) -> StyleProperty {{"));
    w.line("    match property {");
    let value_arm = |w: &mut Writer, p: &Property| {
        let arm = format!(
            "        Property::{} => {}(layer.get_{}()),",
            property_variant(layer, p),
            make_style_property(p),
            snake(&p.name)
        );
        if arm.len() <= MAX_WIDTH {
            w.line(&arm);
        } else {
            w.line(&format!("        Property::{} => {{", property_variant(layer, p)));
            w.line(&format!("            {}(layer.get_{}())", make_style_property(p), snake(&p.name)));
            w.line("        }");
        }
    };
    for p in &paint {
        value_arm(&mut w, p);
    }
    for p in &paint {
        let arm = format!(
            "        Property::{}Transition => make_transition_style_property(&layer.get_{}_transition()),",
            property_variant(layer, p),
            snake(&p.name)
        );
        if arm.len() <= MAX_WIDTH {
            w.line(&arm);
        } else {
            w.line(&format!("        Property::{}Transition => {{", property_variant(layer, p)));
            w.line(&format!("            make_transition_style_property(&layer.get_{}_transition())", snake(&p.name)));
            w.line("        }");
        }
    }
    for p in &layout {
        value_arm(&mut w, p);
    }
    w.line("    }");
    w.line("}");
    w.blank();

    // Layer trait.
    w.line(&format!("impl Layer for {layer_name} {{"));
    w.line("    fn base_impl(&self) -> Arc<dyn layer::Impl> {");
    w.line("        self.base_impl.clone()");
    w.line("    }");
    w.blank();
    w.line("    fn mutate_base_impl(&mut self, f: &mut dyn FnMut(&mut ImplBase)) {");
    w.line("        let mut impl_ = self.mutable_impl();");
    w.line("        f(&mut impl_.base);");
    w.line("        self.set_impl(impl_);");
    w.line("    }");
    w.blank();
    w.line("    fn get_property(&self, name: &str) -> StyleProperty {");
    w.line("        match find_property(name) {");
    w.line("            Some(property) => get_layer_property(self, property),");
    w.line("            None => StyleProperty::default(),");
    w.line("        }");
    w.line("    }");
    w.blank();
    w.line("    fn serialize(&self) -> Value {");
    w.line("        let mut result = serialize_base(self);");
    w.line("        debug_assert!(matches!(result, Value::Object(_)));");
    w.line("        for (name, property) in LAYER_PROPERTIES {");
    w.line("            let style_property = get_layer_property(self, *property);");
    w.line("            if style_property.get_kind() == Kind::Undefined {");
    w.line("                continue;");
    w.line("            }");
    w.line("            let is_paint = (*property as u8) < K_PAINT_PROPERTY_COUNT;");
    w.line("            serialize_property(&mut result, &style_property, name, is_paint);");
    w.line("        }");
    w.line("        result");
    w.line("    }");
    w.blank();
    w.line("    fn clone_ref(&self, id: &str) -> Box<dyn Layer> {");
    w.line("        let mut impl_ = self.mutable_impl();");
    w.line("        impl_.base.id = id.to_owned();");
    w.line(&format!("        impl_.paint = {t}PaintProperties::Transitionable::default();"));
    w.line(&format!("        Box::new({layer_name}::from_impl(Arc::new(impl_)))"));
    w.line("    }");
    w.blank();
    w.line("    fn set_property_internal(&mut self, name: &str, value: &Convertible<'_>) -> Option<Error> {");
    w.line("        let Some(property) = find_property(name) else {");
    w.line("            return Some(Error::new(format!(");
    w.line("                \"layer '{}' doesn't support property '{}'\",");
    w.line("                self.get_id(),");
    w.line("                name");
    w.line("            )));");
    w.line("        };");
    w.blank();
    for (key, members) in conversion_groups(paint.iter().copied().chain(layout.iter().copied())) {
        let terms: Vec<String> =
            members.iter().map(|p| format!("property == Property::{}", property_variant(layer, p))).collect();
        for line in if_condition("        ", &terms) {
            w.line(&line);
        }
        w.line("            let mut error = Error::default();");
        let_else(&mut w, "            ", &key);
        w.blank();
        if members.len() == 1 {
            w.line(&format!("            self.set_{}(&typed_value);", snake(&members[0].name)));
            w.line("            return None;");
        } else {
            for (i, p) in members.iter().enumerate() {
                if i > 0 {
                    w.blank();
                }
                w.line(&format!("            if property == Property::{} {{", property_variant(layer, p)));
                w.line(&format!("                self.set_{}(&typed_value);", snake(&p.name)));
                w.line("                return None;");
                w.line("            }");
            }
        }
        w.line("        }");
        w.blank();
    }
    w.line("        let mut error = Error::default();");
    w.line("        let Some(transition) = <TransitionOptions as Converter>::convert(value, &mut error) else {");
    w.line("            return Some(error);");
    w.line("        };");
    w.blank();
    for p in &paint {
        w.line(&format!("        if property == Property::{}Transition {{", property_variant(layer, p)));
        w.line(&format!("            self.set_{}_transition(&transition);", snake(&p.name)));
        w.line("            return None;");
        w.line("        }");
        w.blank();
    }
    w.line("        Some(Error::new(format!(");
    w.line("            \"layer '{}' doesn't support property '{}'\",");
    w.line("            self.get_id(),");
    w.line("            name");
    w.line("        )))");
    w.line("    }");
    w.line("}");

    w.finish()
}
