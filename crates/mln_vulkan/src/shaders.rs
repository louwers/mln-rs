//! The Vulkan shaders: `mln_glsl`'s GLSL, which naga writes as SPIR-V when a
//! program is first used.

use mln_glsl::{Module, ResourceUse, Stage};
use naga::back::spv;

/// Compiles Vulkan GLSL, whose clip space is y-down, to SPIR-V 1.0, with the
/// entry point `main`.
pub fn compile(stage: Stage, source: &str) -> Result<Vec<u32>, String> {
    write(stage, &Module::parse(stage, source)?, spv::WriterFlags::empty())
}

/// A program's stage compiled to SPIR-V.
pub(crate) struct Compiled {
    pub(crate) spirv: Vec<u32>,
    /// The resources the entry point uses.
    pub(crate) resources: Vec<ResourceUse>,
}

/// Compiles a `ShaderSource::compose`d stage, whose clip space is y-up, and
/// finds the resources it uses.
pub(crate) fn compile_program(stage: Stage, source: &str) -> Result<Compiled, String> {
    let module = Module::parse(stage, source)?;
    let spirv = write(stage, &module, spv::WriterFlags::ADJUST_COORDINATE_SPACE)?;
    Ok(Compiled { spirv, resources: module.resources()? })
}

fn write(stage: Stage, module: &Module, flags: spv::WriterFlags) -> Result<Vec<u32>, String> {
    let options = spv::Options {
        lang_version: (1, 0),
        // The shaders write no fragment depth.
        flags: flags | spv::WriterFlags::LABEL_VARYINGS,
        // No checks added around loops, indexes and integer divisions.
        force_loop_bounding: false,
        emit_int_div_checks: false,
        ..spv::Options::default()
    };
    let pipeline_options = spv::PipelineOptions { shader_stage: stage.naga(), entry_point: "main".to_owned() };
    spv::write_vec(&module.module, &module.info, &options, Some(&pipeline_options)).map_err(|error| format!("{error:?}"))
}
