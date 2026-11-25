//! The symbols' programs, and their collision boxes' and circles'.

use mln_gfx::gfx_types::AttributeDataType::{Float, Float2, Float4, Short2, Short4, UShort2, UShort4};
use mln_gfx::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Property, Stages};
use mln_gfx::shaders::source::Sources;

use super::ubo::{
    CollisionDrawableUBO, CollisionTilePropsUBO, SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};
use crate::shaders::programs::POS;
use crate::terrain::programs::GROUND_DEPTH;

const SYMBOL_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::storage::<SymbolDrawableUBO>("drawableVector", Stages::Vertex),
    ProgramBuffer::storage::<SymbolTilePropsUBO>("tilePropsVector", Stages::Fragment),
    ProgramBuffer::uniform::<SymbolEvaluatedPropsUBO>("props", 0, Stages::Both),
];

const SYMBOL_POS_OFFSET: Attribute = Attribute::new("pos_offset", Short4);

const SYMBOL_DATA: Attribute = Attribute::new("data", UShort4);

const SYMBOL_PIXEL_OFFSET: Attribute = Attribute::new("pixeloffset", Short4);

const SYMBOL_PROJECTED_POS: Attribute = Attribute::new("projected_pos", Float4);

const SYMBOL_FADE_OPACITY: Attribute = Attribute::new("fade_opacity", Float);

const SYMBOL_OPACITY: Attribute = Attribute::new("opacity", Float);

const SYMBOL_FILL_COLOR: Attribute = Attribute::new("fill_color", Float4);

const SYMBOL_HALO_COLOR: Attribute = Attribute::new("halo_color", Float4);

const SYMBOL_HALO_WIDTH: Attribute = Attribute::new("halo_width", Float);

const SYMBOL_HALO_BLUR: Attribute = Attribute::new("halo_blur", Float);

/// The quad's origin in its atlas (`SymbolLayoutVertex::a4`).
const SYMBOL_TEX_ORIGIN: Attribute = Attribute::new("tex_origin", UShort2);

/// The symbol's `symbol-height-offset` (`SymbolLayoutVertex::a5`).
const SYMBOL_HEIGHT_OFFSET: Attribute = Attribute::new("height_offset", Float);

const SYMBOL_SDF_PROPERTIES: &[Property] = &[
    Property::new("opacity"),
    Property::new("fill_color"),
    Property::new("halo_color"),
    Property::new("halo_width"),
    Property::new("halo_blur"),
];

/// The glyphs' or icons' atlas.
const SYMBOL_TEXTURE: ProgramTexture = ProgramTexture::new("texture", Stages::Fragment);

pub static SYMBOL_ICON: Program = Program {
    name: "SymbolIconShader",
    // Only the icons' fragment stage reads the uniform block.
    buffers: &[SYMBOL_BUFFERS[0], SYMBOL_BUFFERS[1], ProgramBuffer { stages: Stages::Fragment, ..SYMBOL_BUFFERS[2] }],
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        SYMBOL_POS_OFFSET,
        SYMBOL_DATA,
        SYMBOL_PIXEL_OFFSET,
        SYMBOL_PROJECTED_POS,
        SYMBOL_FADE_OPACITY,
        SYMBOL_OPACITY,
        SYMBOL_TEX_ORIGIN,
        SYMBOL_HEIGHT_OFFSET,
    ],
    instance_attributes: &[],
    textures: &[SYMBOL_TEXTURE, ProgramTexture::UNUSED, GROUND_DEPTH],
    properties: &[Property::new("opacity")],
    sources: Sources::NONE
        .wgsl(&wgsl::SYMBOL_ICON)
        .msl(&msl::SYMBOL_ICON)
        .glsl(&glsl::SYMBOL_ICON)
        .glsl_es(&glsl_es::SYMBOL_ICON),
};

pub static SYMBOL_SDF: Program = Program {
    name: "SymbolSDFShader",
    buffers: SYMBOL_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        SYMBOL_POS_OFFSET,
        SYMBOL_DATA,
        SYMBOL_PIXEL_OFFSET,
        SYMBOL_PROJECTED_POS,
        SYMBOL_FADE_OPACITY,
        SYMBOL_FILL_COLOR,
        SYMBOL_HALO_COLOR,
        SYMBOL_OPACITY,
        SYMBOL_HALO_WIDTH,
        SYMBOL_HALO_BLUR,
        SYMBOL_TEX_ORIGIN,
        SYMBOL_HEIGHT_OFFSET,
    ],
    instance_attributes: &[],
    textures: &[SYMBOL_TEXTURE, ProgramTexture::UNUSED, GROUND_DEPTH],
    properties: SYMBOL_SDF_PROPERTIES,
    sources: Sources::NONE
        .wgsl(&wgsl::SYMBOL_SDF)
        .msl(&msl::SYMBOL_SDF)
        .glsl(&glsl::SYMBOL_SDF)
        .glsl_es(&glsl_es::SYMBOL_SDF),
};

pub static SYMBOL_TEXT_AND_ICON: Program = Program {
    name: "SymbolTextAndIconShader",
    buffers: SYMBOL_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        SYMBOL_POS_OFFSET,
        SYMBOL_DATA,
        SYMBOL_PROJECTED_POS,
        SYMBOL_FADE_OPACITY,
        SYMBOL_FILL_COLOR,
        SYMBOL_HALO_COLOR,
        SYMBOL_OPACITY,
        SYMBOL_HALO_WIDTH,
        SYMBOL_HALO_BLUR,
        SYMBOL_TEX_ORIGIN,
        SYMBOL_HEIGHT_OFFSET,
    ],
    instance_attributes: &[],
    textures: &[SYMBOL_TEXTURE, ProgramTexture::new("texture_icon", Stages::Fragment), GROUND_DEPTH],
    properties: SYMBOL_SDF_PROPERTIES,
    sources: Sources::NONE
        .wgsl(&wgsl::SYMBOL_TEXT_AND_ICON)
        .msl(&msl::SYMBOL_TEXT_AND_ICON)
        .glsl(&glsl::SYMBOL_TEXT_AND_ICON)
        .glsl_es(&glsl_es::SYMBOL_TEXT_AND_ICON),
};

/// The collision boxes' and circles' buffers, each of a draw's own. The
/// tile properties have the id of a block that both stages read, though
/// only the circles' fragment stage reads them.
const COLLISION_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::draw_uniform::<CollisionDrawableUBO>("drawable", Stages::Vertex),
    ProgramBuffer::draw_uniform::<CollisionTilePropsUBO>("tile_props", Stages::Both),
];

const COLLISION_ANCHOR_POS: Attribute = Attribute::new("anchor_pos", Short2);

const COLLISION_EXTRUDE: Attribute = Attribute::new("extrude", Short2);

const COLLISION_PLACED: Attribute = Attribute::new("placed", UShort2);

pub static COLLISION_BOX: Program = Program {
    name: "CollisionBoxShader",
    buffers: COLLISION_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::Own,
    attributes: &[
        POS,
        COLLISION_ANCHOR_POS,
        COLLISION_EXTRUDE,
        COLLISION_PLACED,
        Attribute::new("shift", Float2),
        // The symbol's `symbol-height-offset` (`CollisionBoxLayoutVertex::a4`).
        Attribute::new("height_offset", Float),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::COLLISION_BOX)
        .msl(&msl::COLLISION_BOX)
        .glsl(&glsl::COLLISION_BOX)
        .glsl_es(&glsl_es::COLLISION_BOX),
};

pub static COLLISION_CIRCLE: Program = Program {
    name: "CollisionCircleShader",
    buffers: COLLISION_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::Own,
    attributes: &[POS, COLLISION_ANCHOR_POS, COLLISION_EXTRUDE, COLLISION_PLACED],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::COLLISION_CIRCLE)
        .msl(&msl::COLLISION_CIRCLE)
        .glsl(&glsl::COLLISION_CIRCLE)
        .glsl_es(&glsl_es::COLLISION_CIRCLE),
};
