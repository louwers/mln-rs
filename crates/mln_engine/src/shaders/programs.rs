//! The engine's programs, and the attributes and textures that many
//! programs read.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Stages};
use mln_gfx::shaders::source::Sources;

use super::ubo::{AtmospherePropsUBO, LayerCompositeUBO, SkyPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

pub(crate) const POS: Attribute = Attribute::new("pos", Short2);

pub(crate) const IMAGE: ProgramTexture = ProgramTexture::new("image", Stages::Fragment);

pub static CLIPPING_MASK: Program = Program {
    name: "ClippingMaskProgram",
    buffers: &[],
    paint_params: None,
    projection: Projection::Own,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::CLIPPING_MASK)
        .msl(&msl::CLIPPING_MASK)
        .glsl(&glsl::CLIPPING_MASK)
        .glsl_es(&glsl_es::CLIPPING_MASK),
};

/// Where a vertex samples the program's textures, in their texels.
pub(crate) const TEXTURE_POS: Attribute = Attribute::new("texture_pos", Short2);

pub static GLOBE_DEPTH: Program = Program {
    name: "GlobeDepthShader",
    buffers: &[],
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::GLOBE_DEPTH)
        .msl(&msl::GLOBE_DEPTH)
        .glsl(&glsl::GLOBE_DEPTH)
        .glsl_es(&glsl_es::GLOBE_DEPTH),
};

pub static SKY: Program = Program {
    name: "SkyShader",
    buffers: &[ProgramBuffer::uniform::<SkyPropsUBO>("sky", 0, Stages::Fragment)],
    paint_params: None,
    projection: Projection::None,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE.wgsl(&wgsl::SKY).msl(&msl::SKY).glsl(&glsl::SKY).glsl_es(&glsl_es::SKY),
};

pub static ATMOSPHERE: Program = Program {
    name: "AtmosphereShader",
    buffers: &[ProgramBuffer::uniform::<AtmospherePropsUBO>("atmosphere", 0, Stages::Both)],
    paint_params: None,
    projection: Projection::None,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::ATMOSPHERE)
        .msl(&msl::ATMOSPHERE)
        .glsl(&glsl::ATMOSPHERE)
        .glsl_es(&glsl_es::ATMOSPHERE),
};

/// A layer drawn into a texture, composited onto the frame.
pub static LAYER_COMPOSITE: Program = Program {
    name: "LayerCompositeShader",
    buffers: &[ProgramBuffer::uniform::<LayerCompositeUBO>("props", 0, Stages::Fragment)],
    paint_params: None,
    projection: Projection::None,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[
        ProgramTexture::texels("image", Stages::Fragment),
        ProgramTexture::texels("backdrop", Stages::Fragment),
    ],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::LAYER_COMPOSITE)
        .msl(&msl::LAYER_COMPOSITE)
        .glsl(&glsl::LAYER_COMPOSITE)
        .glsl_es(&glsl_es::LAYER_COMPOSITE),
};
