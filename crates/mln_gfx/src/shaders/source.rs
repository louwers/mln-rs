//! The code of a program in each shading language a backend compiles, which
//! a [`Program`] carries in its
//! [`Sources`](crate::shaders::program::Sources): [`wgsl`] for
//! `mln_webgpu`, [`msl`] for `mln_mtl`, [`glsl_es`] for `mln_gl`, and
//! [`glsl`] for the backends that compile through `mln_glsl`, `mln_vulkan`
//! and `mln_d3d12`. Each backend composes a program's code with a prelude
//! of its own, of the helpers the shaders share, and with the declarations
//! of the blocks of the engine's buffers and the program's, which it writes
//! from their [`Block`]s: a source declares
//! no block itself.
//!
//! # Properties
//!
//! A variant of a program reads each of its
//! [`properties`](crate::shaders::program::Program::properties) from its
//! uniform where the variant defines `HAS_UNIFORM_u_<name>`, and per vertex
//! otherwise. A source doesn't write both ways out: it names the property in
//! one of these, which the backend writes out for both kinds of variant from
//! the program's [`Property`] and its attribute:
//!
//! - `#property attribute <name>`, in a vertex stage, declares the
//!   attribute at the program's location of it.
//! - `#property varying <name> ...` declares the varying that a vertex
//!   stage passes the values to a fragment stage in.
//! - `#property pass <name>`, in a vertex stage, writes the varying.
//! - A `$` before the property's uniform, as in `let color = $props.color;`,
//!   reads the uniform, or per vertex the attribute in a vertex stage and the
//!   varying in a fragment stage.
//!
//! A directive is a line of its own. The directives write nothing for a
//! variant that reads the uniform, except in GLSL ES, whose varying has
//! the property's own name. What a stage reads per vertex depends on the
//! property's [`PropertyType`]. Each language's module says what the
//! directives write there and what more they take. A source still tests
//! `HAS_UNIFORM_u_<name>` itself where it reads a property another way.

use std::fmt::Write as _;

use crate::gfx_types::AttributeDataType;
use crate::gpu::GpuError;
use crate::shaders::block::Block;
use crate::shaders::program::{Buffer, Program, Property, PropertyType};

pub mod glsl;
pub mod glsl_es;
pub mod msl;
pub mod wgsl;

/// A shader stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
}

/// A property a line of a source names: a directive's or a `$`'s.
pub(crate) struct Named<'a> {
    pub(crate) property: &'a Property,
    /// The location and format of its attribute, if the program has it.
    attribute: Option<(usize, AttributeDataType)>,
    /// A directive's word after the name, but `flat` and a precision.
    value: Option<&'a str>,
    /// Whether a directive says `flat`, after its value.
    pub(crate) flat: bool,
    /// The precision that a directive's last word names: `lowp`, `mediump`
    /// or `highp`.
    pub(crate) precision: Option<&'a str>,
}

impl<'a> Named<'a> {
    pub(crate) fn name(&self) -> &str {
        self.property.name
    }

    pub(crate) fn ty(&self) -> PropertyType {
        self.property.ty
    }

    /// The location of its attribute, which fails if the program has none.
    pub(crate) fn location(&self) -> Result<usize, GpuError> {
        self.attribute
            .map(|(location, _)| location)
            .ok_or_else(|| GpuError::Invalid(format!("no attribute for property {}", self.name())))
    }

    /// Whether its attribute holds integers: a [`PropertyType::Vec4`]'s
    /// of `UShort4`.
    pub(crate) fn integers(&self) -> bool {
        matches!(self.attribute, Some((_, AttributeDataType::UShort4)))
    }

    /// The directive's value, which fails if it has none or, unless `flat`,
    /// says `flat`, or names a precision.
    pub(crate) fn value(&self, flat: bool) -> Result<&'a str, GpuError> {
        self.words(true, flat, false)?;
        Ok(self.value.unwrap_or_default())
    }

    /// Fails unless the directive has a value if `value` and no other, says
    /// `flat` only if `flat`, and names a precision only if `precision`.
    pub(crate) fn words(&self, value: bool, flat: bool, precision: bool) -> Result<(), GpuError> {
        let fits = self.value.is_some() == value && (flat || !self.flat) && (precision || self.precision.is_none());
        fits.then_some(()).ok_or_else(|| GpuError::Invalid(format!("bad arguments for property {}", self.name())))
    }
}

/// The declarations that `declaration` writes of the blocks of `buffers`,
/// each of a storage buffer's element padded to its stride.
pub(crate) fn blocks<'a>(
    buffers: impl IntoIterator<Item = &'a Buffer>,
    declaration: impl Fn(&Block, usize) -> String,
) -> String {
    buffers.into_iter().filter_map(|buffer| Some(declaration(buffer.block()?, buffer.size()))).collect()
}

/// `text` between `#ifndef HAS_UNIFORM_u_<name>` and `#endif`, for the
/// variants that read `named` per vertex.
pub(crate) fn per_vertex(named: &Named<'_>, text: &str) -> String {
    format!("#ifndef HAS_UNIFORM_u_{}\n{text}\n#endif", named.name())
}

/// `text` between `#ifdef HAS_UNIFORM_u_<name>` and `#endif`, for the
/// variants that read `named` from its uniform.
pub(crate) fn from_uniform(named: &Named<'_>, text: &str) -> String {
    format!("#ifdef HAS_UNIFORM_u_{}\n{text}\n#endif", named.name())
}

/// `text` with the property directives and `$`s of `program`'s
/// properties written out (see the [module docs](self)): each directive by
/// `directive`, and each `$`, which names the property after
/// `uniform_prefix` in the last `.`-separated part of the uniform after it,
/// by `value`, which gives what the line reads instead of the uniform per
/// vertex.
pub(crate) fn expand(
    program: &Program,
    text: &str,
    uniform_prefix: &str,
    mut directive: impl FnMut(&str, &Named<'_>) -> Result<String, GpuError>,
    mut value: impl FnMut(&Named<'_>) -> Result<String, GpuError>,
) -> Result<String, GpuError> {
    let find = |name: &str| -> Result<Named<'_>, GpuError> {
        let property = program
            .properties
            .iter()
            .find(|property| property.name == name)
            .ok_or_else(|| GpuError::Invalid(format!("{} has no property {name}", program.name)))?;
        let mix = format!("{name}_t");
        let mixed = matches!(property.ty, PropertyType::Float | PropertyType::Color);
        let blocks = || program.buffers.iter().filter_map(Buffer::block);
        if mixed && !blocks().any(|block| block.members.iter().any(|member| member.name == mix)) {
            return Err(GpuError::Invalid(format!("{} has no block with {mix}, which mixes {name}", program.name)));
        }
        // Its attribute's location: its index, among the instance attributes
        // after the vertex attributes.
        let attribute = program
            .attributes
            .iter()
            .chain(program.instance_attributes)
            .enumerate()
            .find(|(_, attribute)| attribute.id == property.attribute)
            .map(|(location, attribute)| (location, attribute.format));
        Ok(Named { property, attribute, value: None, flat: false, precision: None })
    };
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, newline) = line.strip_suffix('\n').map_or((line, ""), |body| (body, "\n"));
        if let Some(rest) = body.trim_start().strip_prefix("#property ") {
            let mut words = rest.split_whitespace();
            let (Some(role), Some(name)) = (words.next(), words.next()) else {
                return Err(GpuError::Invalid(format!("bad property directive in {}: {body}", program.name)));
            };
            let mut named = find(name)?;
            let mut words: Vec<&str> = words.collect();
            if let Some(&last) = words.last().filter(|last| ["lowp", "mediump", "highp"].contains(last)) {
                named.precision = Some(last);
                words.pop();
            }
            named.flat = words.last() == Some(&"flat");
            if named.flat {
                words.pop();
            }
            named.value = match words[..] {
                [] => None,
                [value] => Some(value),
                _ => return Err(GpuError::Invalid(format!("bad property directive in {}: {body}", program.name))),
            };
            out.push_str(&directive(role, &named)?);
            out.push_str(newline);
        } else if let Some(start) = body.find('$') {
            let after = &body[start + 1..];
            let end = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.')).unwrap_or(after.len());
            let uniform = &after[..end];
            let last = uniform.rsplit('.').next().unwrap_or(uniform);
            let name = last.strip_prefix(uniform_prefix).unwrap_or(last);
            let named = find(name)?;
            let (before, rest) = (&body[..start], &after[end..]);
            if rest.contains('$') {
                return Err(GpuError::Invalid(format!("two properties on a line of {}: {body}", program.name)));
            }
            let per_vertex = value(&named)?;
            _ = write!(
                out,
                "#ifdef HAS_UNIFORM_u_{name}\n{before}{uniform}{rest}\n#else\n{before}{per_vertex}{rest}\n#endif{newline}"
            );
        } else {
            out.push_str(line);
        }
    }
    Ok(out)
}

/// The identifiers that `source`, a shader's code in any of the languages,
/// names outside its `//` and `/* */` comments, and the numbers it writes.
/// A backend declares for a stage the blocks it names.
pub fn identifiers(source: &str) -> impl Iterator<Item = &str> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut rest = source;
    std::iter::from_fn(move || {
        loop {
            rest = rest.trim_start_matches(|c: char| !word(c) && c != '/');
            if let Some(after) = rest.strip_prefix("//") {
                rest = after.find('\n').map_or("", |end| &after[end..]);
            } else if let Some(after) = rest.strip_prefix("/*") {
                rest = after.find("*/").map_or("", |end| &after[end + 2..]);
            } else if let Some(after) = rest.strip_prefix('/') {
                rest = after;
            } else if rest.is_empty() {
                return None;
            } else {
                let end = rest.find(|c: char| !word(c)).unwrap_or(rest.len());
                let (identifier, after) = rest.split_at(end);
                rest = after;
                return Some(identifier);
            }
        }
    })
}

#[cfg(test)]
#[expect(unsafe_code, reason = "`uniform_block!` implements bytemuck's `Pod` for the block it declares")]
mod tests {
    use crate::gfx_types::AttributeDataType::{Float4, Short2};
    use crate::shaders::program::{Attribute, Buffer, Program, Property, Sources, Stages};
    use crate::shaders::source::wgsl::{self, Wgsl};

    crate::uniform_block! {
        pub(crate) struct DotDrawableUBO {
            pub color_t: f32,
            #[padding]
            pub pad1: f32,
            #[padding]
            pub pad2: [f32; 2],
        }
    }

    #[test]
    fn identifiers_skip_comments() {
        let source = "a_b = c / d; // e\n/* f\n g */ h*/";
        assert_eq!(super::identifiers(source).collect::<Vec<_>>(), ["a_b", "c", "d", "h"]);
    }

    static DOT: Program = Program {
        name: "dot",
        buffers: &[Buffer::storage::<DotDrawableUBO>("drawableVector", Stages::Vertex)],
        attributes: &[Attribute { id: 0, format: Short2 }, Attribute { id: 3, format: Float4 }],
        instance_attributes: &[],
        textures: &[],
        properties: &[Property::color("color", 3)],
        sources: Sources::NONE,
    };

    #[test]
    fn directives_write_both_kinds_of_variant() {
        let source = Wgsl {
            vertex: "#property attribute color\n  #property varying color 2 flat\n#property pass color\n",
            fragment: "let color = $props.color * 2.0;",
            prelude: "",
        };
        let [vertex, fragment] = wgsl::expand(&DOT, &source).expect("expands");
        let per_vertex = |text: &str| format!("#ifndef HAS_UNIFORM_u_color\n{text}\n#endif\n");
        assert_eq!(
            vertex,
            [
                per_vertex("@location(1) color: vec4<f32>,"),
                per_vertex("@location(2) @interpolate(flat) color: vec4<f32>,"),
                per_vertex("out.color = unpack_mix_color(in.color, drawable.color_t);"),
            ]
            .concat()
        );
        assert_eq!(
            fragment,
            "#ifdef HAS_UNIFORM_u_color\nlet color = props.color * 2.0;\n#else\nlet color = in.color * 2.0;\n#endif"
        );
    }

    #[test]
    fn a_directive_names_a_property_of_the_program() {
        for vertex in ["#property attribute radius", "let radius = $props.radius;", "#property spin color"] {
            let source = Wgsl { vertex, fragment: "", prelude: "" };
            assert!(wgsl::expand(&DOT, &source).is_err(), "{vertex}");
        }
    }

    #[test]
    fn a_mixed_property_has_its_mixing_factor_in_a_block() {
        let unmixed = Program { name: "unmixed", buffers: &[], ..DOT };
        let source = Wgsl { vertex: "#property attribute color", fragment: "", prelude: "" };
        assert!(wgsl::expand(&unmixed, &source).is_err());
    }
}
