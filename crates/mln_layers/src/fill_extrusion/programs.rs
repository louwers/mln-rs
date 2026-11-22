//! The programs of the `fill-extrusion` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::{Float, Float2, Float4, Short2, Short4, UShort2, UShort4};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Projection, Property, Sources, Stages, Texture};

use super::ubo::{FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

// The ids of the programs' textures and vertex attributes.
pub(crate) const ID_FILL_EXTRUSION_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE: usize = 1;
/// The outline vertex a wall instance starts at.
pub(crate) const ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE: usize = 2;
/// The next outline vertex of a wall instance, which every
/// backend's shaders read as attributes.
pub(crate) const ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE: usize = 3;
pub(crate) const ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE: usize = 4;

// Data-driven properties.
pub(crate) const ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE: usize = 5;
pub(crate) const ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE: usize = 6;
pub(crate) const ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE: usize = 7;
pub(crate) const ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE: usize = 8;
pub(crate) const ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE: usize = 9;

pub(crate) const DRAWABLE: Buffer = Buffer::storage::<FillExtrusionDrawableUBO>("drawableVector", Stages::Vertex);
pub(crate) const TILE_PROPS: Buffer = Buffer::storage::<FillExtrusionTilePropsUBO>("tilePropsVector", Stages::Both);
pub(crate) const PROPS: Buffer = Buffer::uniform::<FillExtrusionPropsUBO>("props", 1);
// The vertex buffers that the pulled walls read as storage buffers.
/// The bucket's vertices: the position and `decimals_ed` of each.
pub(crate) const WALL_VERTICES: Buffer = Buffer::vertices("wallVertices", 0, Short4);
pub(crate) const WALL_PAINT: Buffer = Buffer::vertices("wallPaint", 1, Float);

pub static FILL_EXTRUSION: Program = Program {
    name: "fill-extrusion",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[
        Attribute { id: ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, format: UShort2 },
        Attribute { id: ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::color("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
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
    attributes: &[Attribute { id: ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[
        Attribute { id: ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, format: UShort2 },
        Attribute { id: ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, format: UShort2 },
    ],
    textures: &[],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::color("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
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
    attributes: &[
        Attribute { id: ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, format: UShort2 },
        Attribute { id: ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, format: UShort2 },
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::color("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
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
    buffers: &[DRAWABLE, TILE_PROPS, PROPS, WALL_VERTICES, WALL_PAINT],
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::color("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_PULLED_SHADER)
        .msl(&msl::FILL_EXTRUSION_PULLED_SHADER)
        .glsl(&glsl::FILL_EXTRUSION_PULLED_SHADER),
};

pub static FILL_EXTRUSION_PATTERN: Program = Program {
    name: "fill-extrusion-pattern",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[
        Attribute { id: ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, format: UShort2 },
        Attribute { id: ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, format: UShort4 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
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
    attributes: &[Attribute { id: ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[
        Attribute { id: ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, format: UShort2 },
        Attribute { id: ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, format: UShort2 },
    ],
    textures: &[Texture::new("image")],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
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
        Attribute { id: ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, format: UShort2 },
        Attribute { id: ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, format: UShort2 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER)
        .glsl_es(&glsl_es::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER)
        .glsl(&glsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
};
