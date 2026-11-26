//! The sets of defines that the backends' tests compile each program with.

use std::fmt::Write;

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program::{GLOBE_DEFINE, Program, TERRAIN_DEFINE};

/// A set of defines a program compiles with.
#[derive(Debug)]
pub struct Variant {
    pub name: String,
    /// The projection variant it compiles.
    pub projection: ProjectionVariant,
    /// No property is a uniform: the vertex stage reads every attribute.
    pub per_vertex: bool,
    /// The `#define` lines.
    pub defines: String,
}

/// The `#define` lines of `ProgramParameters(1.0)`, the defines of the
/// projection `variant`, and a uniform define per name.
fn defines<'a>(variant: ProjectionVariant, uniforms: impl IntoIterator<Item = &'a str>) -> String {
    let mut defines = "#define DEVICE_PIXEL_RATIO 1.0\n".to_owned();
    if variant.is_globe() {
        let _ = writeln!(defines, "#define {GLOBE_DEFINE}");
    }
    if variant.stands() {
        let _ = writeln!(defines, "#define {TERRAIN_DEFINE}");
    }
    for name in uniforms {
        let _ = writeln!(defines, "#define HAS_UNIFORM_u_{name}");
    }
    defines
}

/// In each of `projections`, the projection variants of `program`: every
/// property of `program` per vertex and every one as a uniform; with `all`,
/// also each property alone as a uniform.
#[must_use]
pub fn variants(program: &Program, projections: &[ProjectionVariant], all: bool) -> Vec<Variant> {
    let properties: Vec<&str> = program.properties.iter().map(|property| property.name).collect();
    let mut variants = Vec::new();
    for &projection in projections {
        let mut push = |name: &str, per_vertex: bool, defines: String| {
            variants.push(Variant { name: format!("{projection:?}, {name}"), projection, per_vertex, defines });
        };
        push("no uniforms", true, defines(projection, []));
        push("all uniforms", false, defines(projection, properties.iter().copied()));
        if all {
            for &name in &properties {
                push(&format!("uniform {name}"), false, defines(projection, [name]));
            }
        }
    }
    variants
}
