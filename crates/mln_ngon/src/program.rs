use mln_rs::layer::AttributeDataType::{Float, Float2, Float4, Short2};
use mln_rs::layer::{Attribute, Bool32, Program, Projection, Property, Sources, Stages, program};

use crate::{glsl, glsl_es, msl, wgsl};

mln_rs::layer::uniform_block! {
    /// A draw's block: how far a pixel reaches, in the tile or in clip
    /// space, the mix of each property's zoom stops, which the engine
    /// writes, and the radians a pixel spans on the globe.
    pub struct NgonDraw {
        pub extrude_scale: [f32; 2],
        pub color_t: f32,
        pub radius_t: f32,
        pub corners_t: f32,
        pub rotate_t: f32,
        pub blur_t: f32,
        pub opacity_t: f32,
        pub stroke_color_t: f32,
        pub stroke_width_t: f32,
        pub stroke_opacity_t: f32,
        pub globe_extrude_scale: f32,
    }
}

mln_rs::layer::uniform_block! {
    /// The layer's block: each property's value where it is the same for
    /// every feature, which the engine writes, the alignments, and how far
    /// the placement's fade has come
    /// ([`Frame::placement_fade`](mln_rs::layer::Frame::placement_fade)).
    pub(crate) struct NgonProps {
        pub color: [f32; 4],
        pub stroke_color: [f32; 4],
        pub radius: f32,
        pub corners: f32,
        pub rotate: f32,
        pub blur: f32,
        pub opacity: f32,
        pub stroke_width: f32,
        pub stroke_opacity: f32,
        pub scale_with_map: Bool32,
        pub pitch_with_map: Bool32,
        pub fade: f32,
        #[padding]
        pub pad2: f32,
        #[padding]
        pub pad3: f32,
    }
}

/// Draws a polygon over each quad of a point: the quad's corners are the
/// point doubled, with the corner's right and bottom sides in the low bits,
/// each with its opacity as placement left it
/// ([`Opacity::vertex`](mln_rs::layer::placement::Opacity::vertex)).
pub(crate) static NGON: Program = Program {
    name: "ngon",
    buffers: &[
        program::Buffer::storage::<NgonDraw>("draws", Stages::Vertex),
        program::Buffer::uniform::<NgonProps>("props", 0),
    ],
    attributes: &[
        Attribute::new("pos", Short2),
        Attribute::new("color", Float4),
        Attribute::new("radius", Float2),
        Attribute::new("corners", Float2),
        Attribute::new("rotate", Float2),
        Attribute::new("blur", Float2),
        Attribute::new("opacity", Float2),
        Attribute::new("stroke_color", Float4),
        Attribute::new("stroke_width", Float2),
        Attribute::new("stroke_opacity", Float2),
        Attribute::new("fade_opacity", Float),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::color("color"),
        Property::float("radius"),
        Property::float("corners"),
        Property::float("rotate"),
        Property::float("blur"),
        Property::float("opacity"),
        Property::color("stroke_color"),
        Property::float("stroke_width"),
        Property::float("stroke_opacity"),
    ],
    projection: Projection::PerDraw,
    sources: Sources::NONE.wgsl(&wgsl::NGON).msl(&msl::NGON).glsl_es(&glsl_es::NGON).glsl(&glsl::NGON),
};
