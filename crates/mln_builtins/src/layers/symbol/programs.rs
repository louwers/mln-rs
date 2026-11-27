//! The symbols' programs.

use mln_engine::draw::ground::GROUND_DEPTH;
use mln_gfx::gfx_types::AttributeDataType::{Float, Float4, Short4, UShort2, UShort4};
use mln_gfx::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Property, Stages};
use mln_gfx::shaders::source::Sources;

use super::ubo::{SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

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
