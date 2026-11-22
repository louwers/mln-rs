//! The engine's programs and the binding ids they read.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Projection, Sources, Texture};

use super::ubo::{AtmospherePropsUBO, LayerCompositeUBO, SkyPropsUBO, TileDebugPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

pub(crate) const ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE: usize = 0;

pub(crate) const ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE: usize = 0;

/// The corner of the quad that draws a segment: x is 0 at the segment's
/// start and 1 at its end, y picks its side.
pub(crate) const ID_TILE_DEBUG_CORNER_VERTEX_ATTRIBUTE: usize = 0;
/// Per instance, where the segment starts and ends, in tile units.
pub(crate) const ID_TILE_DEBUG_START_ATTRIBUTE: usize = 1;
pub(crate) const ID_TILE_DEBUG_END_ATTRIBUTE: usize = 2;

pub(crate) const ID_LAYER_COMPOSITE_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_LAYER_COMPOSITE_BACKDROP_TEXTURE: usize = 1;
pub(crate) const ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE: usize = 0;

pub(crate) const ID_SKY_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE: usize = 0;

pub(crate) const TILE_DEBUG_PROPS: Buffer = Buffer::uniform::<TileDebugPropsUBO>("props", 0);
pub(crate) const LAYER_COMPOSITE_PROPS: Buffer = Buffer::uniform::<LayerCompositeUBO>("props", 0);
pub(crate) const SKY_PROPS: Buffer = Buffer::uniform::<SkyPropsUBO>("sky", 0);
pub(crate) const ATMOSPHERE_PROPS: Buffer = Buffer::uniform::<AtmospherePropsUBO>("atmosphere", 0);

/// The tile's clipping mask, drawn into the stencil buffer.
pub(crate) static CLIPPING_MASK: Program = Program {
    name: "clipping-mask",
    buffers: &[],
    attributes: &[Attribute { id: ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    projection: Projection::Own,
    sources: Sources::NONE
        .wgsl(&wgsl::CLIPPING_MASK_PROGRAM)
        .msl(&msl::CLIPPING_MASK_PROGRAM)
        .glsl_es(&glsl_es::CLIPPING_MASK_PROGRAM)
        .glsl(&glsl::CLIPPING_MASK_PROGRAM),
};

/// The globe's depth, drawn before its layers.
pub(crate) static GLOBE_DEPTH: Program = Program {
    name: "globe-depth",
    buffers: &[],
    attributes: &[Attribute { id: ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::GLOBE_DEPTH_SHADER)
        .msl(&msl::GLOBE_DEPTH_SHADER)
        .glsl_es(&glsl_es::GLOBE_DEPTH_SHADER)
        .glsl(&glsl::GLOBE_DEPTH_SHADER),
};

/// The style's sky, on a quad over the whole view.
pub(crate) static SKY: Program = Program {
    name: "sky",
    buffers: &[SKY_PROPS],
    attributes: &[Attribute { id: ID_SKY_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    projection: Projection::None,
    sources: Sources::NONE
        .wgsl(&wgsl::SKY_SHADER)
        .msl(&msl::SKY_SHADER)
        .glsl_es(&glsl_es::SKY_SHADER)
        .glsl(&glsl::SKY_SHADER),
};

/// The globe's atmosphere, on a quad over the whole view.
pub(crate) static ATMOSPHERE: Program = Program {
    name: "atmosphere",
    buffers: &[ATMOSPHERE_PROPS],
    attributes: &[Attribute { id: ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    projection: Projection::None,
    sources: Sources::NONE
        .wgsl(&wgsl::ATMOSPHERE_SHADER)
        .msl(&msl::ATMOSPHERE_SHADER)
        .glsl_es(&glsl_es::ATMOSPHERE_SHADER)
        .glsl(&glsl::ATMOSPHERE_SHADER),
};

/// A layer drawn into a texture, composited onto the frame.
pub(crate) static LAYER_COMPOSITE: Program = Program {
    name: "layer-composite",
    buffers: &[LAYER_COMPOSITE_PROPS],
    attributes: &[Attribute { id: ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[],
    textures: &[Texture::unfilterable("image"), Texture::unfilterable("backdrop")],
    properties: &[],
    projection: Projection::None,
    sources: Sources::NONE
        .wgsl(&wgsl::LAYER_COMPOSITE_SHADER)
        .msl(&msl::LAYER_COMPOSITE_SHADER)
        .glsl_es(&glsl_es::LAYER_COMPOSITE_SHADER)
        .glsl(&glsl::LAYER_COMPOSITE_SHADER),
};

/// The engine's debug overlays of tiles: their borders and their text, drawn
/// as segments with round ends, one instance of a quad each. A segment is
/// extruded on the map, in the tile's plane, to the width in pixels of the
/// overlay's uniform block at the frame's zoom (on the globe, at the
/// segment's latitude), with an antialiased edge one device pixel wide.
pub(crate) static TILE_DEBUG: Program = Program {
    name: "tile-debug",
    buffers: &[TILE_DEBUG_PROPS],
    attributes: &[Attribute { id: ID_TILE_DEBUG_CORNER_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[
        Attribute { id: ID_TILE_DEBUG_START_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_TILE_DEBUG_END_ATTRIBUTE, format: Short2 },
    ],
    textures: &[],
    properties: &[],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::TILE_DEBUG_SHADER)
        .msl(&msl::TILE_DEBUG_SHADER)
        .glsl_es(&glsl_es::TILE_DEBUG_SHADER)
        .glsl(&glsl::TILE_DEBUG_SHADER),
};

/// The engine's programs.
#[cfg(test)]
pub(crate) static PROGRAMS: &[&Program] =
    &[&CLIPPING_MASK, &GLOBE_DEPTH, &SKY, &ATMOSPHERE, &LAYER_COMPOSITE, &TILE_DEBUG];
