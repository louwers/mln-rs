//! A program's stages as SPIR-V for Vulkan.

use naga::back::spv;

use crate::{Program, Stage};

impl Program {
    /// `stage` as SPIR-V 1.0 for a Vulkan program, written once, with its
    /// entry point named [`Stage::entry_point`]. Its clip space is Vulkan's,
    /// y-down.
    pub fn spirv(&self, stage: Stage) -> Result<&[u32], String> {
        self.written.spirv[stage as usize].get_or_init(|| write(self, stage)).as_deref().map_err(Clone::clone)
    }
}

fn write(program: &Program, stage: Stage) -> Result<Vec<u32>, String> {
    let options = spv::Options {
        lang_version: (1, 0),
        // The programs write no fragment depth.
        flags: spv::WriterFlags::ADJUST_COORDINATE_SPACE | spv::WriterFlags::LABEL_VARYINGS,
        // No checks added around loops, indexes and integer divisions.
        force_loop_bounding: false,
        emit_int_div_checks: false,
        ..spv::Options::default()
    };
    let pipeline = spv::PipelineOptions { shader_stage: stage.naga(), entry_point: stage.entry_point().to_owned() };
    let validated = program.validated()?;
    spv::write_vec(&validated.module, &validated.info, &options, Some(&pipeline))
        .map_err(|error| format!("{} {stage:?}: {error}", program.name))
}
