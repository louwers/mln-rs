//! The layer types' programs: what their declarations and sources hold
//! that no backend checks.

use std::ptr;

use mln_gfx::shaders::program::Program;

use crate::PROGRAMS;
// slice: off use crate::fill_extrusion::programs::{FILL_EXTRUSION_INSTANCED, FILL_EXTRUSION_PATTERN_INSTANCED};
use crate::symbol::programs::{COLLISION_BOX, COLLISION_CIRCLE};

/// The sources of `program` in the languages of this build: the backends'
/// that the tests build with, as all of the platform's in the workspace's
/// tests.
fn sources(program: &Program) -> Vec<String> {
    let sources = program.sources;
    let wgsl = sources.wgsl.map(|wgsl| [wgsl.prelude, wgsl.vertex, wgsl.fragment].concat());
    let msl = sources.msl.map(|msl| [msl.prelude, msl.source].concat());
    let glsl_es = sources.glsl_es.map(|glsl| [glsl.vertex, glsl.fragment].concat());
    let glsl = sources.glsl.map(|glsl| [glsl.prelude, glsl.vertex, glsl.fragment].concat());
    [wgsl, msl, glsl_es, glsl].into_iter().flatten().collect()
}

/// Every program that draws a layer into the frame writes white for the
/// overdraw inspector, which counts its draws; the collision boxes and
/// circles are a debug overlay, which shows its own colors.
#[test]
fn every_layer_program_shows_the_overdraw_inspector() {
    let overlay = [&COLLISION_BOX, &COLLISION_CIRCLE];
    for &program in PROGRAMS.iter().filter(|&&program| !overlay.iter().any(|&other| ptr::eq(program, other))) {
        let sources = sources(program);
        assert!(!sources.is_empty(), "{program:?} has no source in this build");
        for source in sources {
            assert!(source.contains("OVERDRAW_INSPECTOR"), "{program:?}");
        }
    }
}

// slice: off /// A fill extrusion wall reads its next outline vertex from the buffer of
// slice: off /// its outline vertex, a vertex further on: the last two instance
// slice: off /// attributes, the next outline vertex's, have the formats of the first
// slice: off /// two.
// slice: off #[test]
// slice: off fn a_walls_next_outline_vertex_has_the_formats_of_its_outline_vertex() {
// slice: off     for program in [&FILL_EXTRUSION_INSTANCED, &FILL_EXTRUSION_PATTERN_INSTANCED] {
// slice: off         let formats: Vec<_> = program.instance_attributes.iter().map(|attribute| attribute.format).collect();
// slice: off         assert_eq!(formats[..2], formats[formats.len() - 2..], "{program:?}");
// slice: off     }
// slice: off }
