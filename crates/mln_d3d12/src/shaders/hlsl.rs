// naga writes a program as HLSL for shader model 6.0, with its resources in
// the registers of the root signature (`mln_shaders::hlsl`).

use mln_glsl::{Module, Stage};
pub(crate) use mln_shaders::hlsl::{Hlsl, ROOT_CONSTANT_SPACE, SAMPLER_HEAP_SPACE, SAMPLER_INDEX_REGISTER};

/// Writes a parsed stage of `mln_glsl`'s GLSL as HLSL.
pub(crate) fn write(stage: Stage, module: &Module) -> Result<Hlsl, String> {
    mln_shaders::hlsl::write(&module.module, &module.info, stage.naga(), "main")
}
