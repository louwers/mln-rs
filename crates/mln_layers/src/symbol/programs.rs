//! The programs of the `symbol` layer type: the buffers, attributes,
//! textures and paint properties of the shaders of its symbols and of their
//! collision boxes and circles, and their sources.

use mln_gfx::gfx_types::AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UShort2, UShort4};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Property, Sources, Texture};
use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO,
    ID_PROJECTION_UBO,
};

use super::ubo::{
    CollisionDrawableUBO, CollisionTilePropsUBO, SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_SYMBOL_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_SYMBOL_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO;
pub(crate) const ID_SYMBOL_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_COLLISION_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_COLLISION_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_COLLISION_PROJECTION_UBO: usize = ID_PROJECTION_UBO;
pub(crate) const ID_SYMBOL_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_SYMBOL_IMAGE_ICON_TEXTURE: usize = 1;
pub(crate) const ID_COLLISION_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE: usize = 1;
pub(crate) const ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE: usize = 2;
pub(crate) const ID_COLLISION_PLACED_VERTEX_ATTRIBUTE: usize = 3;
pub(crate) const ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE: usize = 4;
/// The symbol's `symbol-height-offset`.
pub(crate) const ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE: usize = 5;
pub(crate) const ID_SYMBOL_POS_OFFSET_ATTRIBUTE: usize = 0;
pub(crate) const ID_SYMBOL_DATA_ATTRIBUTE: usize = 1;
pub(crate) const ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE: usize = 2;
pub(crate) const ID_SYMBOL_PROJECTED_POS_ATTRIBUTE: usize = 3;
pub(crate) const ID_SYMBOL_FADE_OPACITY_ATTRIBUTE: usize = 4;

// Data-driven properties.
pub(crate) const ID_SYMBOL_OPACITY_ATTRIBUTE: usize = 5;
pub(crate) const ID_SYMBOL_COLOR_ATTRIBUTE: usize = 6;
pub(crate) const ID_SYMBOL_HALO_COLOR_ATTRIBUTE: usize = 7;
pub(crate) const ID_SYMBOL_HALO_WIDTH_ATTRIBUTE: usize = 8;
pub(crate) const ID_SYMBOL_HALO_BLUR_ATTRIBUTE: usize = 9;
/// The atlas origin of a quad's image.
pub(crate) const ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE: usize = 10;
/// The symbol's `symbol-height-offset`.
pub(crate) const ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE: usize = 11;

pub static SYMBOL_ICON: Program = Program {
    name: "symbol-icon",
    buffers: &[
        Buffer::storage::<SymbolDrawableUBO>(ID_SYMBOL_DRAWABLE_UBO),
        Buffer::storage::<SymbolTilePropsUBO>(ID_SYMBOL_TILE_PROPS_UBO),
        Buffer::uniform::<SymbolEvaluatedPropsUBO>(ID_SYMBOL_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_SYMBOL_POS_OFFSET_ATTRIBUTE, "a_pos_offset", Short4),
        Attribute::new(ID_SYMBOL_DATA_ATTRIBUTE, "a_data", UShort4),
        Attribute::new(ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, "a_pixeloffset", Short4),
        Attribute::new(ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, "a_projected_pos", Float3),
        Attribute::new(ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, "a_fade_opacity", Float),
        Attribute::new(ID_SYMBOL_OPACITY_ATTRIBUTE, "a_opacity", Float),
        Attribute::new(ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, "a_tex_origin", UShort2),
        Attribute::new(ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, "a_height_offset", Float),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_texture")],
    properties: &[Property::float("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE)],
    sources: Sources::NONE
        .wgsl(&wgsl::SYMBOL_ICON_SHADER)
        .msl(&msl::SYMBOL_ICON_SHADER)
        .glsl_es(&glsl_es::SYMBOL_ICON_SHADER)
        .glsl(&glsl::SYMBOL_ICON_SHADER),
};

pub static SYMBOL_SDF: Program = Program {
    name: "symbol-sdf",
    buffers: &[
        Buffer::storage::<SymbolDrawableUBO>(ID_SYMBOL_DRAWABLE_UBO),
        Buffer::storage::<SymbolTilePropsUBO>(ID_SYMBOL_TILE_PROPS_UBO),
        Buffer::uniform::<SymbolEvaluatedPropsUBO>(ID_SYMBOL_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_SYMBOL_POS_OFFSET_ATTRIBUTE, "a_pos_offset", Short4),
        Attribute::new(ID_SYMBOL_DATA_ATTRIBUTE, "a_data", UShort4),
        Attribute::new(ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, "a_pixeloffset", Short4),
        Attribute::new(ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, "a_projected_pos", Float3),
        Attribute::new(ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, "a_fade_opacity", Float),
        Attribute::new(ID_SYMBOL_COLOR_ATTRIBUTE, "a_fill_color", Float4),
        Attribute::new(ID_SYMBOL_HALO_COLOR_ATTRIBUTE, "a_halo_color", Float4),
        Attribute::new(ID_SYMBOL_OPACITY_ATTRIBUTE, "a_opacity", Float),
        Attribute::new(ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, "a_halo_width", Float),
        Attribute::new(ID_SYMBOL_HALO_BLUR_ATTRIBUTE, "a_halo_blur", Float),
        Attribute::new(ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, "a_tex_origin", UShort2),
        Attribute::new(ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, "a_height_offset", Float),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_texture")],
    properties: &[
        Property::float("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        Property::color("fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        Property::color("halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        Property::float("halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        Property::float("halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::SYMBOL_SDF_SHADER)
        .msl(&msl::SYMBOL_SDF_SHADER)
        .glsl_es(&glsl_es::SYMBOL_SDF_SHADER)
        .glsl(&glsl::SYMBOL_SDF_SHADER),
};

pub static SYMBOL_TEXT_AND_ICON: Program = Program {
    name: "symbol-text-and-icon",
    buffers: &[
        Buffer::storage::<SymbolDrawableUBO>(ID_SYMBOL_DRAWABLE_UBO),
        Buffer::storage::<SymbolTilePropsUBO>(ID_SYMBOL_TILE_PROPS_UBO),
        Buffer::uniform::<SymbolEvaluatedPropsUBO>(ID_SYMBOL_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_SYMBOL_POS_OFFSET_ATTRIBUTE, "a_pos_offset", Short4),
        Attribute::new(ID_SYMBOL_DATA_ATTRIBUTE, "a_data", UShort4),
        Attribute::new(ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, "a_projected_pos", Float3),
        Attribute::new(ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, "a_fade_opacity", Float),
        Attribute::new(ID_SYMBOL_COLOR_ATTRIBUTE, "a_fill_color", Float4),
        Attribute::new(ID_SYMBOL_HALO_COLOR_ATTRIBUTE, "a_halo_color", Float4),
        Attribute::new(ID_SYMBOL_OPACITY_ATTRIBUTE, "a_opacity", Float),
        Attribute::new(ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, "a_halo_width", Float),
        Attribute::new(ID_SYMBOL_HALO_BLUR_ATTRIBUTE, "a_halo_blur", Float),
        Attribute::new(ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, "a_tex_origin", UShort2),
        Attribute::new(ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, "a_height_offset", Float),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_texture"), Texture::new("u_texture_icon")],
    properties: &[
        Property::float("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        Property::color("fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        Property::color("halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        Property::float("halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        Property::float("halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::SYMBOL_TEXT_AND_ICON_SHADER)
        .msl(&msl::SYMBOL_TEXT_AND_ICON_SHADER)
        .glsl_es(&glsl_es::SYMBOL_TEXT_AND_ICON_SHADER)
        .glsl(&glsl::SYMBOL_TEXT_AND_ICON_SHADER),
};

pub static COLLISION_BOX: Program = Program {
    name: "collision-box",
    buffers: &[
        Buffer::uniform::<CollisionDrawableUBO>(ID_COLLISION_DRAWABLE_UBO),
        Buffer::uniform::<CollisionTilePropsUBO>(ID_COLLISION_TILE_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_COLLISION_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, "a_anchor_pos", Short2),
        Attribute::new(ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, "a_extrude", Short2),
        Attribute::new(ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, "a_placed", UShort2),
        Attribute::new(ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, "a_shift", Float2),
        Attribute::new(ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, "a_height_offset", Float),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::COLLISION_BOX_SHADER)
        .msl(&msl::COLLISION_BOX_SHADER)
        .glsl_es(&glsl_es::COLLISION_BOX_SHADER)
        .glsl(&glsl::COLLISION_BOX_SHADER),
};

pub static COLLISION_CIRCLE: Program = Program {
    name: "collision-circle",
    buffers: &[
        Buffer::uniform::<CollisionDrawableUBO>(ID_COLLISION_DRAWABLE_UBO),
        Buffer::uniform::<CollisionTilePropsUBO>(ID_COLLISION_TILE_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_COLLISION_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, "a_anchor_pos", Short2),
        Attribute::new(ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, "a_extrude", Short2),
        Attribute::new(ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, "a_placed", UShort2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::COLLISION_CIRCLE_SHADER)
        .msl(&msl::COLLISION_CIRCLE_SHADER)
        .glsl_es(&glsl_es::COLLISION_CIRCLE_SHADER)
        .glsl(&glsl::COLLISION_CIRCLE_SHADER),
};
