use mln_builtins::shaders::{every_program, projection_variants};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::GlslSource;
use mln_glsl::variants::variants;

use crate::ShaderCompiler;

/// Every program the renderer draws with that has GLSL, with its GLSL.
fn programs() -> impl Iterator<Item = (&'static Program, &'static GlslSource)> {
    every_program().filter_map(|program| Some((program, program.sources.glsl?)))
}

/// The first bytes of a DXIL container.
const DXIL_MAGIC: &[u8] = b"DXBC";

fn compiler() -> ShaderCompiler {
    ShaderCompiler::new().unwrap_or_else(|error| panic!("{error}: put DXC's dxcompiler.dll on the DLL search path"))
}

// Every program compiles to DXIL, with every property per vertex, every
// property as a uniform, and each property alone as a uniform, in both
// projections.
#[test]
fn every_program_compiles_to_dxil() {
    let compiler = compiler();
    let mut failures = Vec::new();
    let mut compiled = 0;
    for (program, source) in programs() {
        for variant in variants(program, projection_variants(program), true) {
            let at = format!("{} ({})", program.name, variant.name);
            match compiler.compile_source(program, source, &variant.defines) {
                Ok(program) if program.vertex.starts_with(DXIL_MAGIC) && program.fragment.starts_with(DXIL_MAGIC) => {
                    compiled += 1;
                }
                Ok(_) => failures.push(format!("{at}: no DXIL")),
                Err(error) => failures.push(format!("{at}:\n{error}")),
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} programs failed:\n{}", failures.len(), compiled + failures.len(), {
        failures.join("\n\n")
    });
}

// Only the pulled programs read the vertex index, and so the base vertex in
// naga's special constants.
#[test]
fn only_the_pulled_programs_read_the_vertex_index() {
    let compiler = compiler();
    for (program, source) in programs() {
        for variant in variants(program, projection_variants(program), false) {
            let compiled = compiler.compile_source(program, source, &variant.defines).expect("the program");
            let pulled = program.name.contains("Pulled");
            assert_eq!(compiled.reads_vertex_index, pulled, "{} ({})", program.name, variant.name);
        }
    }
}
