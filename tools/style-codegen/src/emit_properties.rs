// Ports src/mln/style/layers/layer_properties.hpp.ejs and
// layer_properties.cpp.ejs: emits crates/mln_style/src/layers/<type>_layer_properties.rs.

use crate::defaults::{default_value, rust_value};
use crate::spec::{LayerSpec, Property};
use crate::types::{
    camelize, camelize_with_leading_lowercase, evaluated_type, expression_type_rust, is_color_ramp, is_data_driven,
    is_overridable, layout_property_type_cpp, paint_property_type_cpp, snake,
};
use crate::writer::{MAX_WIDTH, Writer, imports_for, layer_file_name};

/// Layer types whose `LayerProperties` hold a `CrossfadeParameters`.
pub fn has_crossfade(type_: &str) -> bool {
    matches!(type_, "background" | "fill" | "line" | "fill-extrusion")
}

/// The C++ tag declaration, as the doc comment of the Rust tag (inside the
/// macro call, which passes it on).
fn cpp_tag_comment(w: &mut Writer, property: &Property, base: &str, layout: bool) {
    let t = evaluated_type(property);
    if is_color_ramp(property) {
        w.line(&format!("    /// `struct {} : {base} {{}};`", camelize(&property.name)));
        return;
    }
    w.line(&format!("    /// `struct {} : {base}`:", camelize(&property.name)));
    if layout {
        w.line(&format!("    /// `static constexpr const char *name() {{ return \"{}\"; }}`,", property.name));
    }
    let end = if is_overridable(property) { "," } else { "." };
    w.line(&format!(
        "    /// `static {} defaultValue() {{ return {}; }}`{end}",
        t.cpp(),
        default_value(property).cpp()
    ));
    if is_overridable(property) {
        w.line(&format!("    /// `static constexpr const char *name() {{ return \"{}\"; }}`, ...", property.name));
    }
}

fn emit_tag(w: &mut Writer, property: &Property, layer_type: &str, layout: bool) {
    let name = camelize(&property.name);
    let t = evaluated_type(property);
    if layout {
        let macro_name = if is_data_driven(property) { "data_driven_layout_property" } else { "layout_property" };
        w.line(&format!("crate::{macro_name}!("));
        cpp_tag_comment(w, property, &layout_property_type_cpp(property), true);
        let default = rust_value(&default_value(property), &t);
        let decl = format!("    pub struct {name}: {} = {default}, name = \"{}\"", t.rust(), property.name);
        if decl.len() <= MAX_WIDTH {
            w.line(&decl);
        } else {
            w.line(&format!("    pub struct {name}: {} = {default},", t.rust()));
            w.line(&format!("    name = \"{}\"", property.name));
        }
        w.line(");");
        w.blank();
        return;
    }

    let macro_name = match property.property_type() {
        Some("color-ramp") => "color_ramp_property",
        Some("data-driven") => "data_driven_paint_property",
        Some("cross-faded-data-driven") => "cross_faded_data_driven_paint_property",
        Some("cross-faded") => "cross_faded_paint_property",
        _ => "paint_property",
    };
    w.line(&format!("crate::{macro_name}!("));
    cpp_tag_comment(w, property, &paint_property_type_cpp(property, layer_type), false);
    if is_color_ramp(property) {
        w.line(&format!("    pub struct {name}"));
    } else {
        let default = rust_value(&default_value(property), &t);
        let mut options = String::new();
        if is_overridable(property) {
            options.push_str(", overridable = true");
        }
        if property.name == "line-floor-width" {
            // `using EvaluatorType = DataDrivenPropertyEvaluator<float, true>;`
            options.push_str(", use_integer_zoom = true");
        }
        w.line(&format!("    pub struct {name}: {} = {default}{options}", t.rust()));
    }
    w.line(");");
    if is_overridable(property) {
        w.blank();
        w.line(&format!("impl {name} {{"));
        w.line("    /// `static constexpr const char *name()`.");
        w.line("    pub const fn name() -> &'static str {");
        w.line(&format!("        \"{}\"", property.name));
        w.line("    }");
        w.blank();
        w.line("    /// `static constexpr auto expressionType()`.");
        w.line("    pub fn expression_type() -> Type {");
        w.line(&format!("        {}", expression_type_rust(property.type_(), property.value())));
        w.line("    }");
        w.blank();
        w.line(&format!(
            "    /// `template<typename T> static bool hasOverride(const T& t) {{ return !!t.{}; }}`.",
            camelize_with_leading_lowercase(&property.name)
        ));
        w.line("    pub fn has_override<T: OverridableSection>(t: &T) -> bool {");
        w.line(&format!("        t.has_{}()", snake(&property.name)));
        w.line("    }");
        w.line("}");
    }
    w.blank();
}

fn emit_set(w: &mut Writer, macro_name: &str, class: &str, properties: &[Property]) {
    w.line(&format!("crate::{macro_name}!("));
    w.line(&format!("    /// `class {class} : public Properties<...>`, the properties in field order."));
    w.line("    #[allow(non_snake_case)]");
    w.line(&format!("    pub mod {class} {{"));
    for p in properties {
        w.line(&format!("        {}: {},", snake(&p.name), camelize(&p.name)));
    }
    w.line("    }");
    w.line(");");
    w.blank();
}

pub fn emit(layer: &LayerSpec) -> String {
    let t = camelize(&layer.type_);
    let file = layer_file_name(&layer.type_);
    let crossfade = has_crossfade(&layer.type_);
    let has_layout = !layer.layout_properties.is_empty();
    let overridable = layer.paint_properties.iter().any(is_overridable);
    let mut w = Writer::new();

    w.line("// This file is generated by //tools/style-codegen from the style spec");
    w.line("// (scripts/style-spec-reference/v8.json). Do not edit.");
    w.line("//");
    w.line(&format!("// Ports src/mln/style/layers/{file}_layer_properties.hpp +"));
    w.line(&format!("// src/mln/style/layers/{file}_layer_properties.cpp (generated in C++ from"));
    w.line("// src/mln/style/layers/layer_properties.{hpp,cpp}.ejs).");
    w.blank();

    let mut imports = imports_for(layer.layout_properties.iter().chain(&layer.paint_properties), false);
    imports.push("crate::expression::Dependency".into());
    if overridable {
        imports.push("crate::expression::Type".into());
        imports.push("crate::properties::OverridableSection".into());
    }
    let mut layer_properties_import = vec!["HasEvaluated", "LayerProperties", "LayerPropertiesBase"];
    if crossfade {
        layer_properties_import.insert(0, "HasCrossfade");
    }
    imports.push(format!("crate::layer_properties::{{{}}}", layer_properties_import.join(", ")));
    imports.push(format!("crate::layers::{file}_layer_impl::Impl as {t}LayerImpl"));
    if crossfade {
        imports.push("crate::renderer::property_evaluation_parameters::CrossfadeParameters".into());
    }
    imports.push("mln_util::immutable::Immutable".into());
    imports.push("mln_vec_map::VecSet".into());
    w.imports(imports);
    w.blank();

    for p in &layer.layout_properties {
        emit_tag(&mut w, p, &layer.type_, true);
    }
    for p in &layer.paint_properties {
        emit_tag(&mut w, p, &layer.type_, false);
    }

    if has_layout {
        emit_set(&mut w, "layout_properties", &format!("{t}LayoutProperties"), &layer.layout_properties);
    }
    emit_set(&mut w, "paint_properties", &format!("{t}PaintProperties"), &layer.paint_properties);

    let pe = format!("{t}PaintProperties::PossiblyEvaluated");
    w.line(&format!("/// `{t}LayerProperties final : public LayerProperties`."));
    w.line("#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]");
    w.line(&format!("pub struct {t}LayerProperties {{"));
    w.line("    pub base: LayerPropertiesBase,");
    w.line("    // Data members.");
    if crossfade {
        w.line("    pub crossfade: CrossfadeParameters,");
    }
    w.line(&format!("    pub evaluated: {pe},"));
    w.line("}");
    w.blank();

    w.line(&format!("impl {t}LayerProperties {{"));
    w.line(&format!("    /// `explicit {t}LayerProperties(Immutable<{t}Layer::Impl>)`."));
    w.line(&format!("    pub fn new(impl_: Immutable<{t}LayerImpl>) -> Self {{"));
    w.line(&format!("        {t}LayerProperties {{"));
    w.line("            base: LayerPropertiesBase::new(impl_),");
    if crossfade {
        w.line("            crossfade: CrossfadeParameters::default(),");
    }
    w.line(&format!("            evaluated: {pe}::default(),"));
    w.line("        }");
    w.line("    }");
    w.blank();
    if crossfade {
        w.line(&format!(
            "    /// `{t}LayerProperties(Immutable<{t}Layer::Impl>, CrossfadeParameters, {t}PaintProperties::PossiblyEvaluated)`."
        ));
        w.line("    pub fn with_evaluated(");
        w.line(&format!("        impl_: Immutable<{t}LayerImpl>,"));
        w.line("        crossfade: CrossfadeParameters,");
        w.line(&format!("        evaluated: {pe},"));
        w.line("    ) -> Self {");
        w.line(&format!("        {t}LayerProperties {{"));
        w.line("            base: LayerPropertiesBase::new(impl_),");
        w.line("            crossfade,");
        w.line("            evaluated,");
        w.line("        }");
        w.line("    }");
    } else {
        w.line(&format!(
            "    /// `{t}LayerProperties(Immutable<{t}Layer::Impl>, {t}PaintProperties::PossiblyEvaluated)`."
        ));
        w.line("    pub fn with_evaluated(");
        w.line(&format!("        impl_: Immutable<{t}LayerImpl>,"));
        w.line(&format!("        evaluated: {pe},"));
        w.line("    ) -> Self {");
        w.line(&format!("        {t}LayerProperties {{"));
        w.line("            base: LayerPropertiesBase::new(impl_),");
        w.line("            evaluated,");
        w.line("        }");
        w.line("    }");
    }
    w.blank();
    w.line(&format!("    pub fn layer_impl(&self) -> &{t}LayerImpl {{"));
    w.line("        self.base");
    w.line("            .base_impl");
    w.line(&format!("            .downcast_ref::<{t}LayerImpl>()"));
    w.line(&format!("            .expect(\"{t}LayerProperties: not a {} layer impl\")", layer.type_));
    w.line("    }");
    w.line("}");
    w.blank();

    w.line(&format!("impl LayerProperties for {t}LayerProperties {{"));
    w.line("    fn base(&self) -> &LayerPropertiesBase {");
    w.line("        &self.base");
    w.line("    }");
    w.blank();
    w.line("    fn base_mut(&mut self) -> &mut LayerPropertiesBase {");
    w.line("        &mut self.base");
    w.line("    }");
    w.blank();
    w.line("    fn constants_mask(&self) -> u64 {");
    w.line("        self.evaluated.constants_mask()");
    w.line("    }");
    w.blank();
    w.line("    fn get_dependencies(&self) -> Dependency {");
    if has_layout {
        w.line("        self.layer_impl().paint.get_dependencies() | self.layer_impl().layout.get_dependencies()");
    } else {
        w.line("        self.layer_impl().paint.get_dependencies()");
    }
    w.line("    }");
    w.blank();
    w.line("    fn get_evaluated_dependencies(&self) -> Dependency {");
    w.line("        self.evaluated.get_dependencies()");
    w.line("    }");
    w.blank();
    w.line("    fn collect_evaluated_global_state_refs(&self, refs: &mut VecSet<String>) {");
    w.line("        self.evaluated.collect_global_state_refs(refs);");
    w.line("    }");
    w.line("}");
    w.blank();

    w.line(&format!("impl HasEvaluated for {t}LayerProperties {{"));
    w.line(&format!("    type Evaluated = {pe};"));
    w.blank();
    w.line("    fn evaluated(&self) -> &Self::Evaluated {");
    w.line("        &self.evaluated");
    w.line("    }");
    w.line("}");
    if crossfade {
        w.blank();
        w.line(&format!("impl HasCrossfade for {t}LayerProperties {{"));
        w.line("    fn crossfade(&self) -> &CrossfadeParameters {");
        w.line("        &self.crossfade");
        w.line("    }");
        w.line("}");
    }

    w.finish()
}
