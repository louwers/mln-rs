// Compiles GLSL to SPIR-V, in place of the glslang calls of
// src/mln/shaders/vulkan/shader_program.cpp (`compileGlsl`).
//
// naga's GLSL front end takes GLSL 4.50 for Vulkan, less what
// `glsl_sources` avoids: combined image samplers, `bool` members of
// buffers, precision qualifiers and samplers as function parameters.

use naga::ShaderStage;
use naga::back::spv;
use naga::front::glsl;
use naga::valid::{Capabilities, ValidationFlags, Validator};

use super::glsl_sources::{PRELUDE_FRAGMENT, PRELUDE_VERTEX};

/// The GLSL version every source is compiled as (`defaultVersion`).
const GLSL_VERSION: u32 = 450;

/// The stages of a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
}

impl Stage {
    fn naga(self) -> ShaderStage {
        match self {
            Stage::Vertex => ShaderStage::Vertex,
            Stage::Fragment => ShaderStage::Fragment,
        }
    }

    /// The prelude of every shader of the stage
    /// (`ShaderSource<BuiltIn::Prelude, Vulkan>`).
    fn prelude(self) -> &'static str {
        match self {
            Stage::Vertex => PRELUDE_VERTEX,
            Stage::Fragment => PRELUDE_FRAGMENT,
        }
    }
}

/// The source glslang is given for a stage: the version, the program's
/// defines (`#define` lines), the stage's prelude, and the shader's own
/// prelude and source.
#[must_use]
pub fn compose(stage: Stage, defines: &str, shader_prelude: &str, source: &str) -> String {
    format!("#version {GLSL_VERSION}\n{defines}\n{}\n{shader_prelude}\n{source}", stage.prelude())
}

/// Parses a composed source into a naga module.
pub fn parse(stage: Stage, source: &str) -> Result<naga::Module, String> {
    let mut frontend = glsl::Frontend::default();
    frontend.parse(&glsl::Options::from(stage.naga()), source).map_err(|errors| errors.emit_to_string(source))
}

/// Compiles a composed source to SPIR-V 1.0, with the entry point `main`.
pub fn compile(stage: Stage, source: &str) -> Result<Vec<u32>, String> {
    let module = parse(stage, source)?;
    // Not the uniformity of control flow: it is WGSL's rule for derivatives,
    // which GLSL doesn't have.
    let flags = ValidationFlags::all() & !ValidationFlags::CONTROL_FLOW_UNIFORMITY;
    let info = Validator::new(flags, Capabilities::all()).validate(&module).map_err(|error| format!("{error:?}"))?;

    let options = spv::Options {
        lang_version: (1, 0),
        // The sources flip Y themselves (`applySurfaceTransform`) and write
        // no fragment depth.
        flags: spv::WriterFlags::LABEL_VARYINGS,
        // As glslang compiles them: no checks added around loops, indexes
        // and integer divisions.
        force_loop_bounding: false,
        emit_int_div_checks: false,
        ..spv::Options::default()
    };
    let pipeline_options = spv::PipelineOptions { shader_stage: stage.naga(), entry_point: "main".to_owned() };
    spv::write_vec(&module, &info, &options, Some(&pipeline_options)).map_err(|error| format!("{error:?}"))
}
