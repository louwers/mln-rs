//! The sets of defines that the backends' tests compile each program with.

use std::fmt::Write;

use mln_gfx::shaders::program::{GLOBE_DEFINE, Program};

#[derive(Clone, Copy)]
enum Projection {
    Mercator,
    Globe,
}

#[derive(Clone, Copy)]
enum Overdraw {
    Off,
    On,
}

/// A set of defines a program compiles with.
#[derive(Debug)]
pub struct Variant {
    pub name: String,
    /// No property is a uniform: the vertex stage reads every attribute.
    pub per_vertex: bool,
    /// The `#define` lines.
    pub defines: String,
}

/// The projection variants of a program: its name.
const PROJECTIONS: [(&str, Projection); 2] = [("Mercator", Projection::Mercator), ("globe", Projection::Globe)];

/// The `#define` lines of `ProgramParameters(1.0, overdraw)`, the globe
/// define if `Projection::Globe`, and a uniform define per name.
fn defines<'a>(overdraw: Overdraw, projection: Projection, uniforms: impl IntoIterator<Item = &'a str>) -> String {
    let mut defines = "#define DEVICE_PIXEL_RATIO 1.0\n".to_owned();
    if matches!(overdraw, Overdraw::On) {
        defines.push_str("#define OVERDRAW_INSPECTOR\n");
    }
    if matches!(projection, Projection::Globe) {
        let _ = writeln!(defines, "#define {GLOBE_DEFINE}");
    }
    for name in uniforms {
        let _ = writeln!(defines, "#define HAS_UNIFORM_u_{name}");
    }
    defines
}

/// In both projections: every property of `program` per vertex and every one
/// as a uniform; with `all`, also these two with the overdraw inspector, and
/// each property alone as a uniform.
#[must_use]
pub fn variants(program: &Program, all: bool) -> Vec<Variant> {
    let properties: Vec<&str> = program.properties.iter().map(|property| property.name).collect();
    let mut variants = Vec::new();
    for (projection, proj) in PROJECTIONS {
        let mut push = |name: &str, per_vertex: bool, defines: String| {
            variants.push(Variant { name: format!("{projection}, {name}"), per_vertex, defines });
        };
        push("no uniforms", true, defines(Overdraw::Off, proj, []));
        push("all uniforms", false, defines(Overdraw::Off, proj, properties.iter().copied()));
        if all {
            push("overdraw inspector", true, defines(Overdraw::On, proj, []));
            push("overdraw inspector, all uniforms", false, defines(Overdraw::On, proj, properties.iter().copied()));
            for &name in &properties {
                push(&format!("uniform {name}"), false, defines(Overdraw::Off, proj, [name]));
            }
        }
    }
    variants
}
