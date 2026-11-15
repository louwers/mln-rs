use mln_glsl::variants::variants;
use mln_glsl::{PROGRAMS, Stage};

use crate::shaders::compile_program;

/// The first word of a SPIR-V module.
const SPIRV_MAGIC: u32 = 0x0723_0203;

// Every stage compiles to a SPIR-V module, with every property per vertex,
// every property as a uniform, each property alone as a uniform, and with
// the overdraw inspector, in both projections.
#[test]
fn every_program_compiles_to_spirv() {
    let mut failures = Vec::new();
    let mut compiled = 0;
    for &(_, source) in PROGRAMS {
        for variant in variants(source, true) {
            for stage in [Stage::Vertex, Stage::Fragment] {
                let at = format!("{} ({}, {stage:?})", source.name, variant.name);
                match compile_program(stage, &source.compose(stage, &variant.defines)) {
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
