//! The programs of the `symbol` layer type: the attributes, textures and
//! paint properties of the shaders of its symbols and of their collision
//! boxes and circles.

use mln_gfx::gfx_types::AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UShort2, UShort4};
use mln_gfx::shaders::program::{Program, Property};
use mln_gfx::shaders::shader_defines::{
    ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE,
    ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE,
    ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, ID_SYMBOL_COLOR_ATTRIBUTE,
    ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, ID_SYMBOL_HALO_BLUR_ATTRIBUTE,
    ID_SYMBOL_HALO_COLOR_ATTRIBUTE, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE,
    ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_OPACITY_ATTRIBUTE,
    ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE,
    ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE,
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
