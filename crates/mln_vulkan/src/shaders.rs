//! GLSL compiled to SPIR-V for a host's own shaders, such as an overlay's.
//! The renderer's programs are `mln_shaders`', which writes their SPIR-V.

use mln_shaders::Stage;
use naga::back::spv;
use naga::valid::{Capabilities, ValidationFlags, Validator};

/// Compiles a stage of Vulkan GLSL 4.50, whose entry point is `main` and
/// whose clip space is y-down, to SPIR-V 1.0 with the entry point `main`.
/// naga's GLSL front end takes no combined image samplers, `bool` members of
/// buffers, precision qualifiers or samplers as function parameters.
pub fn compile(stage: Stage, source: &str) -> Result<Vec<u32>, String> {
    let module = naga::front::glsl::Frontend::default()
        .parse(&naga::front::glsl::Options::from(stage.naga()), source)
        .map_err(|errors| errors.emit_to_string(source))?;
    // Not the uniformity of control flow: it is WGSL's rule for derivatives,
    // which GLSL doesn't have.
    let flags = ValidationFlags::all() & !ValidationFlags::CONTROL_FLOW_UNIFORMITY;
    let info = Validator::new(flags, Capabilities::all()).validate(&module).map_err(|error| format!("{error:?}"))?;
    let options = spv::Options {
        lang_version: (1, 0),
        flags: spv::WriterFlags::LABEL_VARYINGS,
        // No checks added around loops, indexes and integer divisions.
        force_loop_bounding: false,
        emit_int_div_checks: false,
        ..spv::Options::default()
    };
    let pipeline_options = spv::PipelineOptions { shader_stage: stage.naga(), entry_point: "main".to_owned() };
    spv::write_vec(&module, &info, &options, Some(&pipeline_options)).map_err(|error| format!("{error:?}"))
}
