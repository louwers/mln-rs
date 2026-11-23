use mln_rs::layer::AttributeDataType::Short2;
use mln_rs::layer::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Sources};

use crate::{glsl, glsl_es, msl, wgsl};

mln_rs::layer::uniform_block! {
    /// A pass's block: how it samples its input, and the operations it
    /// applies then, in order.
    pub struct Process {
        /// xy: a texel of what the pass draws, in uv; z: how many
        /// operations it applies.
        pub output: [f32; 4],
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
/// `image` and, for a combine, the target `other`.
pub(crate) static PROCESS: Program = Program {
    name: "image-process",
    buffers: &[ProgramBuffer::uniform::<Process>("process", 0)],
    attributes: &[Attribute::new("pos", Short2)],
    instance_attributes: &[],
    textures: &[ProgramTexture::new("image"), ProgramTexture::new("other")],
    properties: &[],
    projection: Projection::None,
    sources: Sources::NONE.wgsl(&wgsl::PROCESS).msl(&msl::PROCESS).glsl_es(&glsl_es::PROCESS).glsl(&glsl::PROCESS),
};
