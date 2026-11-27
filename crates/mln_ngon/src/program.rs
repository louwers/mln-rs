//! The polygons' program and the blocks it reads.

use mln_engine::draw::ground::{GROUND_DEPTH, GROUND_HEIGHT};
use mln_engine::shaders::programs::POS;
use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4};
use mln_gfx::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Property, Stages};
use mln_gfx::shaders::source::Sources;

use crate::{glsl, glsl_es, msl, wgsl};

mln_gfx::uniform_block! {
    /// A draw's block: its tile's matrix, how far a pixel reaches in the
    /// tile or in clip space, the mix of each data-driven property's zoom
    /// stops, and the radians a pixel spans on the globe.
    pub struct NgonDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub extrude_scale: [f32; 2],
        pub radius_t: f32,
        pub corners_t: f32,
        pub rotate_t: f32,
        pub color_t: f32,
        pub blur_t: f32,
        pub opacity_t: f32,
        pub stroke_width_t: f32,
        pub stroke_color_t: f32,
        pub stroke_opacity_t: f32,
        pub globe_extrude_scale: f32,
    }
}

mln_gfx::uniform_block! {
    /// The layer's block: each property's value where it is the same for
    /// every feature, the colors premultiplied, and the alignments.
    pub struct NgonEvaluatedPropsUBO {
        pub color: [f32; 4],
        pub stroke_color: [f32; 4],
        pub radius: f32,
        pub corners: f32,
        pub rotate: f32,
        pub blur: f32,
        pub opacity: f32,
        pub stroke_width: f32,
        pub stroke_opacity: f32,
        pub scale_with_map: i32,
        pub pitch_with_map: i32,
        pub pad1: f32,
        pub pad2: f32,
        pub pad3: f32,
    }
}

/// Draws a polygon over each quad of a point: the quad's corners are the
/// point doubled, with the corner's right and bottom sides in the low bits.
pub(crate) static NGON: Program = Program {
    name: "NgonShader",
    buffers: &[
        ProgramBuffer::storage::<NgonDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::uniform::<NgonEvaluatedPropsUBO>("props", 0, Stages::Both),
    ],
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        Attribute::new("radius", Float2),
        Attribute::new("corners", Float2),
        Attribute::new("rotate", Float2),
        Attribute::new("color", Float4),
        Attribute::new("blur", Float2),
        Attribute::new("opacity", Float2),
        Attribute::new("stroke_width", Float2),
        Attribute::new("stroke_color", Float4),
        Attribute::new("stroke_opacity", Float2),
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, GROUND_DEPTH],
    properties: &[
        Property::new("radius"),
        Property::new("corners"),
        Property::new("rotate"),
        Property::new("color"),
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("stroke_width"),
        Property::new("stroke_color"),
        Property::new("stroke_opacity"),
    ],
    sources: Sources::NONE.wgsl(&wgsl::NGON).msl(&msl::NGON).glsl(&glsl::NGON).glsl_es(&glsl_es::NGON),
};
