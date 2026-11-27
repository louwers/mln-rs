//! The parts' program and the block it reads.

use mln_engine::draw::ground::GROUND_DEPTH;
use mln_engine::shaders::programs::POS;
use mln_gfx::shaders::program::{Program, ProgramBuffer, ProgramTexture, Projection, Stages};
use mln_gfx::shaders::source::Sources;

use crate::{glsl, glsl_es, msl, wgsl};

mln_gfx::uniform_block! {
    /// A part's block: its shape and colors, and where its anchor and its
    /// quad lie on the ground, in the tile units of the anchor it draws at.
    pub struct PartDraw {
        /// The shape ([`Shape`](crate::parts::Shape)), then what it reads:
        /// a disc's border width, in device pixels or, where `w` is 1, as a
        /// share of its radius, and a sector's half width in radians.
        pub style: [f32; 4],
        /// The fill, premultiplied.
        pub fill: [f32; 4],
        /// A disc's border, premultiplied.
        pub border: [f32; 4],
        /// The anchor's point, and the height of the ground under it in
        /// meters, which every part stands at and which the ground in front
        /// of the point hides.
        pub ground: [f32; 4],
        /// The shape's center.
        pub center: [f32; 2],
        /// Where its unit square's x and y axes point, as long as its radius.
        pub axis_x: [f32; 2],
        pub axis_y: [f32; 2],
        pub pad1: f32,
        pub pad2: f32,
    }
}

/// Draws a part's shape over a quad around its center: the quad's corners
/// are its unit square's.
pub(crate) static PUCK: Program = Program {
    name: "LocationPuckShader",
    buffers: &[ProgramBuffer::storage::<PartDraw>("drawableVector", Stages::Vertex)],
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, GROUND_DEPTH],
    properties: &[],
    sources: Sources::NONE.wgsl(&wgsl::PUCK).msl(&msl::PUCK).glsl(&glsl::PUCK).glsl_es(&glsl_es::PUCK),
};
