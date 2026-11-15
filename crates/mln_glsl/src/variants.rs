//! The sets of defines that the backends' tests compile each program with.

use std::fmt::Write;

use mln_gfx::shader_group::GLOBE_DEFINE;

use crate::ShaderSource;

/// A set of defines a program compiles with.
#[derive(Debug)]
pub struct Variant {
    pub name: String,
    /// No property is a uniform: the vertex stage reads every attribute.
    pub per_vertex: bool,
    /// The `#define` lines.
    pub defines: String,
}

/// The projection variants a shader group builds: its name, and whether it
/// defines `PROJECTION_GLOBE`.
const PROJECTIONS: [(&str, bool); 2] = [("Mercator", false), ("globe", true)];

/// The `#define` lines of `ProgramParameters(1.0, overdraw)`, the globe
/// define if `globe`, and a uniform define per name.
fn defines<'a>(overdraw: bool, globe: bool, uniforms: impl IntoIterator<Item = &'a &'a str>) -> String {
    let mut defines = "#define DEVICE_PIXEL_RATIO 1.0\n".to_owned();
    if overdraw {
        defines.push_str("#define OVERDRAW_INSPECTOR\n");
    }
    if globe {
        let _ = writeln!(defines, "#define {GLOBE_DEFINE}");
    }
    for name in uniforms {
        let _ = writeln!(defines, "#define HAS_UNIFORM_u_{name}");
    }
    defines
}

/// In both projections: every property per vertex and every property as a
/// uniform; with `all`, also these two with the overdraw inspector, and each
/// property alone as a uniform.
#[must_use]
pub fn variants(source: &ShaderSource, all: bool) -> Vec<Variant> {
    let properties = source.uniform_properties();
    let mut variants = Vec::new();
    for (projection, globe) in PROJECTIONS {
        let mut push = |name: &str, per_vertex: bool, defines: String| {
            variants.push(Variant { name: format!("{projection}, {name}"), per_vertex, defines });
        };
        push("no uniforms", true, defines(false, globe, []));
        push("all uniforms", false, defines(false, globe, &properties));
        if all {
            push("overdraw inspector", true, defines(true, globe, []));
            push("overdraw inspector, all uniforms", false, defines(true, globe, &properties));
            for name in &properties {
                push(&format!("uniform {name}"), false, defines(false, globe, [name]));
            }
        }
    }
    variants
}
