use mln_glsl::PROGRAMS;
use mln_glsl::variants::variants;

use crate::ShaderCompiler;

/// The first bytes of a DXIL container.
const DXIL_MAGIC: &[u8] = b"DXBC";

fn compiler() -> ShaderCompiler {
    ShaderCompiler::new().unwrap_or_else(|error| panic!("{error}: put DXC's dxcompiler.dll on the DLL search path"))
}

// Every program compiles to DXIL, with every property per vertex, every
// property as a uniform, each property alone as a uniform, and with the
// overdraw inspector, in both projections.
#[test]
fn every_program_compiles_to_dxil() {
    let compiler = compiler();
    let mut failures = Vec::new();
    let mut compiled = 0;
    for &(_, source) in PROGRAMS {
        for variant in variants(source, true) {
            let at = format!("{} ({})", source.name, variant.name);
            match compiler.compile_source(source, &variant.defines) {
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
