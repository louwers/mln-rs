use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::glsl::Glsl;
use mln_glsl::variants::variants;
use mln_glsl::{Stage, compose};

use crate::shaders::compile_program;

/// Each program of the built-in layer types with GLSL, and the
/// conformance cases' own programs.
fn programs() -> Vec<(&'static Program, &'static Glsl)> {
    let programs = mln_layers::PROGRAMS.iter().chain(&mln_layers::conformance::PROGRAMS).copied();
    programs.filter_map(|program| Some((program, program.sources.glsl?))).collect()
}

/// The first word of a SPIR-V module.
const SPIRV_MAGIC: u32 = 0x0723_0203;

// Every stage compiles to a SPIR-V module, with every property per vertex,
// every property as a uniform, each property alone as a uniform, and with
// the overdraw inspector, in both projections.
#[test]
fn every_program_compiles_to_spirv() {
    let mut failures = Vec::new();
    let mut compiled = 0;
    for (program, source) in programs() {
        for variant in variants(program, true) {
            for stage in [Stage::Vertex, Stage::Fragment] {
                let at = format!("{} ({}, {stage:?})", program.name, variant.name);
                match compile_program(stage, &compose(program, source, stage, &variant.defines).expect("expands")) {
                    Ok(compiled_stage) if compiled_stage.spirv.first() == Some(&SPIRV_MAGIC) => compiled += 1,
                    Ok(_) => failures.push(format!("{at}: no SPIR-V")),
                    Err(error) => failures.push(format!("{at}:\n{error}")),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} stages failed:\n{}", failures.len(), compiled + failures.len(), {
        failures.join("\n\n")
    });
}
