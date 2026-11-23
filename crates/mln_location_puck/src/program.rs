use mln_rs::layer::AttributeDataType::Short2;
use mln_rs::layer::{Attribute, Program, ProgramBuffer, Projection, Sources, Stages};

use crate::{glsl, glsl_es, msl, wgsl};

mln_rs::layer::uniform_block! {
    /// A part's block: its shape and colors, and where its quad lies on the
    /// ground, in the tile units of the anchor it draws at.
    pub struct PartDraw {
        /// The shape ([`Shape`](crate::parts::Shape)), then what it reads:
        /// a disc's border width, in device pixels or, where `w` is 1, as a
        /// share of its radius, and a sector's half width in radians.
        pub style: [f32; 4],
        /// The fill, premultiplied.
        pub fill: [f32; 4],
        /// A disc's border, premultiplied.
        pub border: [f32; 4],
        /// The shape's center.
        pub center: [f32; 2],
        /// Where its unit square's x and y axes point, as long as its radius.
        pub axis_x: [f32; 2],
        pub axis_y: [f32; 2],
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
    }
}

/// Draws a part's shape over a quad around its center: the quad's corners
/// are its unit square's.
pub(crate) static PUCK: Program = Program {
    name: "location-puck",
    buffers: &[ProgramBuffer::storage::<PartDraw>("draws", Stages::Vertex)],
    attributes: &[Attribute::new("pos", Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    projection: Projection::PerDraw,
    sources: Sources::NONE.wgsl(&wgsl::PUCK).msl(&msl::PUCK).glsl_es(&glsl_es::PUCK).glsl(&glsl::PUCK),
};
