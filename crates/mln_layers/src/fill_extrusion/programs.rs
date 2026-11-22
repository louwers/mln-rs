//! The programs of the `fill-extrusion` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.
//!
//! The roofs draw the bucket's triangles. The walls draw a quad from each
//! outline vertex to the next: as instances of one quad, an instance per
//! vertex that reads the next vertex too; as quads of their own in a vertex
//! buffer, where instances are slow; or, where the backend binds vertex
//! buffers as storage buffers, as quads without attributes, whose shader
//! reads the bucket's vertices and paint values by index.

use mln_gfx::gfx_types::AttributeDataType::{Float, Float2, Float4, Short2, Short4, UShort2, UShort4};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Projection, Property, Sources, Stages, Texture};

use super::ubo::{FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

const DRAWABLE: Buffer = Buffer::storage::<FillExtrusionDrawableUBO>("drawableVector", Stages::Vertex);
const TILE_PROPS: Buffer = Buffer::storage::<FillExtrusionTilePropsUBO>("tilePropsVector", Stages::Both);
const PROPS: Buffer = Buffer::uniform::<FillExtrusionPropsUBO>("props", 1);

/// The corner of the wall quad, or a roof vertex's position.
const POS: Attribute = Attribute::new("pos", Short2);
/// An outline vertex's normal and edge distance.
const DECIMALS_ED: Attribute = Attribute::new("decimals_ed", UShort2);
/// The outline vertex a wall starts at, and the next one.
const OUTLINE_POS: Attribute = Attribute::new("outline_pos", Short2);
const NEXT_OUTLINE_POS: Attribute = Attribute::new("next_outline_pos", Short2);
const NEXT_DECIMALS_ED: Attribute = Attribute::new("next_decimals_ed", UShort2);
const COLOR: Attribute = Attribute::new("color", Float4);
const BASE: Attribute = Attribute::new("base", Float2);
const HEIGHT: Attribute = Attribute::new("height", Float2);
const PATTERN_FROM: Attribute = Attribute::new("pattern_from", UShort4);
const PATTERN_TO: Attribute = Attribute::new("pattern_to", UShort4);

const PLAIN: &[Property] = &[Property::float("base"), Property::color("color"), Property::float("height")];
const PATTERNED: &[Property] = &[
    Property::float("base"),
    Property::float("height"),
    Property::vec4("pattern_from"),
    Property::vec4("pattern_to"),
];

/// The walls' attributes of an instance, or of each vertex of a wall quad.
const WALL: &[Attribute] = &[OUTLINE_POS, DECIMALS_ED, COLOR, BASE, HEIGHT, NEXT_OUTLINE_POS, NEXT_DECIMALS_ED];
const PATTERN_WALL: &[Attribute] =
    &[OUTLINE_POS, DECIMALS_ED, BASE, HEIGHT, PATTERN_FROM, PATTERN_TO, NEXT_OUTLINE_POS, NEXT_DECIMALS_ED];

pub static FILL_EXTRUSION: Program = Program {
    name: "fill-extrusion",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[POS, DECIMALS_ED, COLOR, BASE, HEIGHT],
    instance_attributes: &[],
    textures: &[],
    properties: PLAIN,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_SHADER)
        .msl(&msl::FILL_EXTRUSION_SHADER)
        .glsl_es(&glsl_es::FILL_EXTRUSION_SHADER)
        .glsl(&glsl::FILL_EXTRUSION_SHADER),
};

pub static FILL_EXTRUSION_INSTANCED: Program = Program {
    name: "fill-extrusion-instanced",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[POS],
    instance_attributes: WALL,
    textures: &[],
    properties: PLAIN,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_INSTANCED_SHADER)
        .msl(&msl::FILL_EXTRUSION_INSTANCED_SHADER)
        .glsl_es(&glsl_es::FILL_EXTRUSION_INSTANCED_SHADER)
        .glsl(&glsl::FILL_EXTRUSION_INSTANCED_SHADER),
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] with every attribute per
/// vertex, for walls whose quads are in the vertex buffer.
pub static FILL_EXTRUSION_WALL: Program = Program {
    name: "fill-extrusion-wall",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[POS, OUTLINE_POS, DECIMALS_ED, COLOR, BASE, HEIGHT, NEXT_OUTLINE_POS, NEXT_DECIMALS_ED],
    instance_attributes: &[],
    textures: &[],
    properties: PLAIN,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_INSTANCED_SHADER)
        .glsl_es(&glsl_es::FILL_EXTRUSION_INSTANCED_SHADER)
        .glsl(&glsl::FILL_EXTRUSION_INSTANCED_SHADER),
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its wall's outline vertices and paint values
/// from storage buffers, by its index.
pub static FILL_EXTRUSION_PULLED: Program = Program {
    name: "fill-extrusion-pulled",
    buffers: &[
        DRAWABLE,
        TILE_PROPS,
        PROPS,
        // The bucket's vertices: the position and `decimals_ed` of each.
        Buffer::vertices("wallVertices", 0, Short4),
        Buffer::vertices("wallPaint", 1, Float),
    ],
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: PLAIN,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_PULLED_SHADER)
        .msl(&msl::FILL_EXTRUSION_PULLED_SHADER)
        .glsl(&glsl::FILL_EXTRUSION_PULLED_SHADER),
};

pub static FILL_EXTRUSION_PATTERN: Program = Program {
    name: "fill-extrusion-pattern",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[POS, DECIMALS_ED, BASE, HEIGHT, PATTERN_FROM, PATTERN_TO],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: PATTERNED,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_PATTERN_SHADER)
        .msl(&msl::FILL_EXTRUSION_PATTERN_SHADER)
        .glsl_es(&glsl_es::FILL_EXTRUSION_PATTERN_SHADER)
        .glsl(&glsl::FILL_EXTRUSION_PATTERN_SHADER),
};

pub static FILL_EXTRUSION_PATTERN_INSTANCED: Program = Program {
    name: "fill-extrusion-pattern-instanced",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[POS],
    instance_attributes: PATTERN_WALL,
    textures: &[Texture::new("image")],
    properties: PATTERNED,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER)
        .msl(&msl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER)
        .glsl_es(&glsl_es::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER)
        .glsl(&glsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
};

/// [`FILL_EXTRUSION_WALL`] of the pattern walls.
pub static FILL_EXTRUSION_PATTERN_WALL: Program = Program {
    name: "fill-extrusion-pattern-wall",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[
        POS,
        OUTLINE_POS,
        DECIMALS_ED,
        BASE,
        HEIGHT,
        PATTERN_FROM,
        PATTERN_TO,
        NEXT_OUTLINE_POS,
        NEXT_DECIMALS_ED,
    ],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: PATTERNED,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER)
        .glsl_es(&glsl_es::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER)
        .glsl(&glsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
};
