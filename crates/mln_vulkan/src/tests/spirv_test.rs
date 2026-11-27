use mln_builtins::shaders::{every_program, projection_variants};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::GlslSource;
use mln_glsl::variants::variants;
use mln_glsl::{Stage, compose};

use crate::shaders::compile_program;

/// Every program the renderer draws with that has GLSL, with its GLSL.
fn programs() -> impl Iterator<Item = (&'static Program, &'static GlslSource)> {
    every_program().filter_map(|program| Some((program, program.sources.glsl?)))
}

/// The first word of a SPIR-V module.
const SPIRV_MAGIC: u32 = 0x0723_0203;

// Every stage compiles to a SPIR-V module, with every property per vertex,
// every property as a uniform, and each property alone as a uniform, in both
// projections.
#[test]
fn every_program_compiles_to_spirv() {
    let mut failures = Vec::new();
    let mut compiled = 0;
    for (program, source) in programs() {
        for variant in variants(program, projection_variants(program), true) {
            for stage in [Stage::Vertex, Stage::Fragment] {
                let at = format!("{} ({}, {stage:?})", program.name, variant.name);
                match compose(source, program, stage, &variant.defines)
                    .and_then(|composed| compile_program(stage, &composed))
                {
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
