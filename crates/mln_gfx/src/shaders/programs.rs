//! The engine's programs (`CLIPPING_MASK`, `SKY`, `ATMOSPHERE`,
//! `GLOBE_DEPTH`, `LAYER_COMPOSITE` and `TILE_DEBUG`), and the symbol layer
//! type's, which is in the engine. Each backend has their sources.

use crate::gfx_types::AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UByte4, UShort2, UShort4};
use crate::shaders::program::{Program, Property};
use crate::shaders::shader_defines::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE,
    ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE,
    ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE,
    ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE,
    ID_LAYER_COMPOSITE_BACKDROP_TEXTURE, ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE,
    ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_SKY_POS_VERTEX_ATTRIBUTE,
    ID_SYMBOL_COLOR_ATTRIBUTE, ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE,
    ID_SYMBOL_HALO_BLUR_ATTRIBUTE, ID_SYMBOL_HALO_COLOR_ATTRIBUTE, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE,
    ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE,
    ID_SYMBOL_OPACITY_ATTRIBUTE, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE,
    ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE,
};

pub static CLIPPING_MASK: Program = Program {
    name: "ClippingMaskProgram",
    attributes: &[(ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static SYMBOL_ICON: Program = Program {
    name: "SymbolIconShader",
    attributes: &[
        (ID_SYMBOL_POS_OFFSET_ATTRIBUTE, Short4),
        (ID_SYMBOL_DATA_ATTRIBUTE, UShort4),
        (ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, Short4),
        (ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, Float3),
        (ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, UShort2),
        (ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, Float),
    ],
    instance_attributes: &[],
    textures: &[ID_SYMBOL_IMAGE_TEXTURE],
    properties: &[Property::new("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE)],
};

pub static SYMBOL_SDF: Program = Program {
    name: "SymbolSDFShader",
    attributes: &[
        (ID_SYMBOL_POS_OFFSET_ATTRIBUTE, Short4),
        (ID_SYMBOL_DATA_ATTRIBUTE, UShort4),
        (ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, Short4),
        (ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, Float3),
        (ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_COLOR_ATTRIBUTE, Float4),
        (ID_SYMBOL_HALO_COLOR_ATTRIBUTE, Float4),
        (ID_SYMBOL_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, Float),
        (ID_SYMBOL_HALO_BLUR_ATTRIBUTE, Float),
        (ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, UShort2),
        (ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, Float),
    ],
    instance_attributes: &[],
    textures: &[ID_SYMBOL_IMAGE_TEXTURE],
    properties: &[
        Property::new("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        Property::new("fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        Property::new("halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        Property::new("halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        Property::new("halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    ],
};

pub static SYMBOL_TEXT_AND_ICON: Program = Program {
    name: "SymbolTextAndIconShader",
    attributes: &[
        (ID_SYMBOL_POS_OFFSET_ATTRIBUTE, Short4),
        (ID_SYMBOL_DATA_ATTRIBUTE, UShort4),
        (ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, Float3),
        (ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_COLOR_ATTRIBUTE, Float4),
        (ID_SYMBOL_HALO_COLOR_ATTRIBUTE, Float4),
        (ID_SYMBOL_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, Float),
        (ID_SYMBOL_HALO_BLUR_ATTRIBUTE, Float),
        (ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, UShort2),
        (ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, Float),
    ],
    instance_attributes: &[],
    textures: &[ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_IMAGE_ICON_TEXTURE],
    properties: &[
        Property::new("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        Property::new("fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        Property::new("halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        Property::new("halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        Property::new("halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    ],
};

pub static COLLISION_BOX: Program = Program {
    name: "CollisionBoxShader",
    attributes: &[
        (ID_COLLISION_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, UShort2),
        (ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, Float2),
        (ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, Float),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static COLLISION_CIRCLE: Program = Program {
    name: "CollisionCircleShader",
    attributes: &[
        (ID_COLLISION_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, UShort2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static GLOBE_DEPTH: Program = Program {
    name: "GlobeDepthShader",
    attributes: &[(ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static SKY: Program = Program {
    name: "SkyShader",
    attributes: &[(ID_SKY_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static ATMOSPHERE: Program = Program {
    name: "AtmosphereShader",
    attributes: &[(ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

/// A layer drawn into a texture, composited onto the frame.
pub static LAYER_COMPOSITE: Program = Program {
    name: "LayerCompositeShader",
    attributes: &[(ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE],
    properties: &[],
};

/// The engine's debug overlays of tiles: their borders and their text, drawn
/// as lines whose color, width and other properties are uniforms
/// ([`tile_debug_ubo`](crate::shaders::tile_debug_ubo)).
pub static TILE_DEBUG: Program = Program {
    name: "TileDebugShader",
    attributes: &[(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2), (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};
