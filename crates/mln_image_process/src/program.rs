//! The passes' program and the block it reads.

use mln_engine::shaders::programs::POS;
use mln_gfx::shaders::program::{Program, ProgramBuffer, ProgramTexture, Projection, Stages};
use mln_gfx::shaders::source::Sources;

use crate::{glsl, glsl_es, msl, wgsl};

mln_gfx::uniform_block! {
    /// A pass's block: how it samples its input, and the operations it then
    /// applies, in order.
    pub struct ImageProcessUBO {
        /// xy: a texel of what the pass draws, in uv; z: how many operations
        /// it applies.
        pub texel: [f32; 4],
        /// xy: a texel of the input along the blur's axis, in uv; z: the
        /// blur's taps each side, none for a copy; w: the weight of the
        /// texel at the center.
        pub blur: [f32; 4],
        /// The blur's taps, each its offset in texels and its weight, two
        /// to a vector.
        pub taps: [[f32; 4]; 16],
        /// The operations, two vectors each: the kind (1 threshold, 2 color,
        /// 3 opacity, 4 combine) and its parameter (a threshold's cutoff,
        /// from 0 to 1, an opacity, or the combine's mode), then a color's
        /// premultiplied value.
        pub ops: [[f32; 4]; 16],
    }
}

/// Draws a pass over the whole of what it draws into, from its input
/// `image` and, for a combine, the shared target `other`, sampled at the
/// fragment's place in what it draws.
pub(crate) static IMAGE_PROCESS: Program = Program {
    name: "ImageProcessShader",
    buffers: &[ProgramBuffer::uniform::<ImageProcessUBO>("process", 0, Stages::Fragment)],
    paint_params: None,
    projection: Projection::None,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[ProgramTexture::new("image", Stages::Fragment), ProgramTexture::new("other", Stages::Fragment)],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::IMAGE_PROCESS)
        .msl(&msl::IMAGE_PROCESS)
        .glsl(&glsl::IMAGE_PROCESS)
        .glsl_es(&glsl_es::IMAGE_PROCESS),
};
