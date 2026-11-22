//! The programs of the `symbol` layer type: the buffers, attributes,
//! textures and paint properties of the shaders of its symbols and of their
//! collision boxes and circles, and their sources.

use mln_gfx::gfx_types::AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UShort2, UShort4};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Projection, Property, Sources, Stages, Texture};

use super::ubo::{
    CollisionDrawableUBO, CollisionTilePropsUBO, SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

const DRAWABLE: Buffer = Buffer::storage::<SymbolDrawableUBO>("drawableVector", Stages::Vertex);
const TILE_PROPS: Buffer = Buffer::storage::<SymbolTilePropsUBO>("tilePropsVector", Stages::Fragment);
const PROPS: Buffer = Buffer::uniform::<SymbolEvaluatedPropsUBO>("props", 0);
const COLLISION_DRAWABLE: Buffer = Buffer::draw_uniform::<CollisionDrawableUBO>("drawable", Stages::Vertex);
const COLLISION_TILE_PROPS: Buffer = Buffer::draw_uniform::<CollisionTilePropsUBO>("tileProps", Stages::Both);

pub static SYMBOL_ICON: Program = Program {
    name: "symbol-icon",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[
        Attribute::new("pos_offset", Short4),
        Attribute::new("data", UShort4),
        Attribute::new("pixeloffset", Short4),
        Attribute::new("projected_pos", Float3),
        Attribute::new("fade_opacity", Float),
        Attribute::new("opacity", Float),
        Attribute::new("tex_origin", UShort2),
        Attribute::new("height_offset", Float),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[Property::float("opacity")],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::SYMBOL_ICON_SHADER)
        .msl(&msl::SYMBOL_ICON_SHADER)
        .glsl_es(&glsl_es::SYMBOL_ICON_SHADER)
        .glsl(&glsl::SYMBOL_ICON_SHADER),
};

pub static SYMBOL_SDF: Program = Program {
    name: "symbol-sdf",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[
        Attribute::new("pos_offset", Short4),
        Attribute::new("data", UShort4),
        Attribute::new("pixeloffset", Short4),
        Attribute::new("projected_pos", Float3),
        Attribute::new("fade_opacity", Float),
        Attribute::new("fill_color", Float4),
        Attribute::new("halo_color", Float4),
        Attribute::new("opacity", Float),
        Attribute::new("halo_width", Float),
        Attribute::new("halo_blur", Float),
        Attribute::new("tex_origin", UShort2),
        Attribute::new("height_offset", Float),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[
        Property::float("opacity"),
        Property::color("fill_color"),
        Property::color("halo_color"),
        Property::float("halo_width"),
        Property::float("halo_blur"),
    ],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::SYMBOL_SDF_SHADER)
        .msl(&msl::SYMBOL_SDF_SHADER)
        .glsl_es(&glsl_es::SYMBOL_SDF_SHADER)
        .glsl(&glsl::SYMBOL_SDF_SHADER),
};

pub static SYMBOL_TEXT_AND_ICON: Program = Program {
    name: "symbol-text-and-icon",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[
        Attribute::new("pos_offset", Short4),
        Attribute::new("data", UShort4),
        Attribute::new("projected_pos", Float3),
        Attribute::new("fade_opacity", Float),
        Attribute::new("fill_color", Float4),
        Attribute::new("halo_color", Float4),
        Attribute::new("opacity", Float),
        Attribute::new("halo_width", Float),
        Attribute::new("halo_blur", Float),
        Attribute::new("tex_origin", UShort2),
        Attribute::new("height_offset", Float),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("glyph_image"), Texture::new("icon_image")],
    properties: &[
        Property::float("opacity"),
        Property::color("fill_color"),
        Property::color("halo_color"),
        Property::float("halo_width"),
        Property::float("halo_blur"),
    ],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::SYMBOL_TEXT_AND_ICON_SHADER)
        .msl(&msl::SYMBOL_TEXT_AND_ICON_SHADER)
        .glsl_es(&glsl_es::SYMBOL_TEXT_AND_ICON_SHADER)
        .glsl(&glsl::SYMBOL_TEXT_AND_ICON_SHADER),
};

pub static COLLISION_BOX: Program = Program {
    name: "collision-box",
    buffers: &[COLLISION_DRAWABLE, COLLISION_TILE_PROPS],
    attributes: &[
        Attribute::new("pos", Short2),
        Attribute::new("anchor_pos", Short2),
        Attribute::new("extrude", Short2),
        Attribute::new("placed", UShort2),
        Attribute::new("shift", Float2),
        Attribute::new("height_offset", Float),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    projection: Projection::Own,
    sources: Sources::NONE
        .wgsl(&wgsl::COLLISION_BOX_SHADER)
        .msl(&msl::COLLISION_BOX_SHADER)
        .glsl_es(&glsl_es::COLLISION_BOX_SHADER)
        .glsl(&glsl::COLLISION_BOX_SHADER),
};

pub static COLLISION_CIRCLE: Program = Program {
    name: "collision-circle",
    buffers: &[COLLISION_DRAWABLE, COLLISION_TILE_PROPS],
    attributes: &[
        Attribute::new("pos", Short2),
        Attribute::new("anchor_pos", Short2),
        Attribute::new("extrude", Short2),
        Attribute::new("placed", UShort2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    projection: Projection::Own,
    sources: Sources::NONE
        .wgsl(&wgsl::COLLISION_CIRCLE_SHADER)
        .msl(&msl::COLLISION_CIRCLE_SHADER)
        .glsl_es(&glsl_es::COLLISION_CIRCLE_SHADER)
        .glsl(&glsl::COLLISION_CIRCLE_SHADER),
};
