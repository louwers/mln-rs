//! The fill extrusions' programs.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort2, UShort4};
use mln_gfx::shaders::program::{
    Attribute, Program, ProgramBuffer, Projection, Property, Stages, Variants, VertexWord,
};
use mln_gfx::shaders::source::Sources;

use super::ubo::{FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};
use crate::render::terrain::programs::GROUND_HEIGHT;
use crate::shaders::programs::{IMAGE, POS};

/// The fill extrusions' buffers. The uniform block is at slot 1, which
/// leaves slot 0 for the pattern's tiles. The patterns' fragment stages read
/// the uniform block too.
const FILL_EXTRUSION_DRAWABLE: ProgramBuffer =
    ProgramBuffer::storage::<FillExtrusionDrawableUBO>("drawableVector", Stages::Vertex);

const FILL_EXTRUSION_PROPS: ProgramBuffer = ProgramBuffer::uniform::<FillExtrusionPropsUBO>("props", 1, Stages::Vertex);

const FILL_EXTRUSION_BUFFERS: &[ProgramBuffer] = &[FILL_EXTRUSION_DRAWABLE, FILL_EXTRUSION_PROPS];

const FILL_EXTRUSION_PATTERN_BUFFERS: &[ProgramBuffer] = &[
    FILL_EXTRUSION_DRAWABLE,
    ProgramBuffer::storage::<FillExtrusionTilePropsUBO>("tilePropsVector", Stages::Both),
    ProgramBuffer { stages: Stages::Both, ..FILL_EXTRUSION_PROPS },
];

const FILL_EXTRUSION_DECIMALS_ED: Attribute = Attribute::new("decimals_ed", UShort2);

const FILL_EXTRUSION_BASE: Attribute = Attribute::new("base", Float2);

const FILL_EXTRUSION_COLOR: Attribute = Attribute::new("color", Float4);

const FILL_EXTRUSION_HEIGHT: Attribute = Attribute::new("height", Float2);

const FILL_EXTRUSION_PATTERN_FROM: Attribute = Attribute::new("pattern_from", UShort4);

const FILL_EXTRUSION_PATTERN_TO: Attribute = Attribute::new("pattern_to", UShort4);

/// A wall instance's outline vertex, and the outline vertex after it.
const FILL_EXTRUSION_OUTLINE_POS: Attribute = Attribute::new("outline_pos", Short2);

const FILL_EXTRUSION_NEXT_OUTLINE_POS: Attribute = Attribute::new("next_outline_pos", Short2);

const FILL_EXTRUSION_NEXT_DECIMALS_ED: Attribute = Attribute::new("next_decimals_ed", UShort2);

const FILL_EXTRUSION_PROPERTIES: &[Property] =
    &[Property::new("base"), Property::new("color"), Property::new("height")];

const FILL_EXTRUSION_PATTERN_PROPERTIES: &[Property] =
    &[Property::new("base"), Property::new("height"), Property::new("pattern_from"), Property::new("pattern_to")];

pub static FILL_EXTRUSION: Program = Program {
    name: "FillExtrusionShader",
    buffers: FILL_EXTRUSION_BUFFERS,
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_COLOR,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_EXTRUSION_PROPERTIES,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION)
        .msl(&msl::FILL_EXTRUSION)
        .glsl(&glsl::FILL_EXTRUSION)
        .glsl_es(&glsl_es::FILL_EXTRUSION),
};

pub static FILL_EXTRUSION_INSTANCED: Program = Program {
    name: "FillExtrusionInstancedShader",
    buffers: FILL_EXTRUSION_BUFFERS,
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[
        FILL_EXTRUSION_OUTLINE_POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_COLOR,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        FILL_EXTRUSION_NEXT_OUTLINE_POS,
        FILL_EXTRUSION_NEXT_DECIMALS_ED,
        GROUND_HEIGHT,
    ],
    textures: &[],
    properties: FILL_EXTRUSION_PROPERTIES,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_INSTANCED)
        .msl(&msl::FILL_EXTRUSION_INSTANCED)
        .glsl(&glsl::FILL_EXTRUSION_INSTANCED)
        .glsl_es(&glsl_es::FILL_EXTRUSION_INSTANCED),
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] with every attribute per
/// vertex, for walls whose quads are in the vertex buffer.
pub static FILL_EXTRUSION_WALL: Program = Program {
    name: "FillExtrusionWallShader",
    buffers: FILL_EXTRUSION_BUFFERS,
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        FILL_EXTRUSION_OUTLINE_POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_COLOR,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        FILL_EXTRUSION_NEXT_OUTLINE_POS,
        FILL_EXTRUSION_NEXT_DECIMALS_ED,
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_EXTRUSION_PROPERTIES,
    // The instanced walls' text with every attribute per vertex. No MSL:
    // Metal leaves out the walls of devices that rasterize on the CPU.
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_INSTANCED)
        .glsl(&glsl::FILL_EXTRUSION_INSTANCED)
        .glsl_es(&glsl_es::FILL_EXTRUSION_INSTANCED),
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its wall's outline vertices and paint values
/// from storage buffers, by its index.
pub static FILL_EXTRUSION_PULLED: Program = Program {
    name: "FillExtrusionPulledShader",
    buffers: &[
        FILL_EXTRUSION_BUFFERS[0],
        FILL_EXTRUSION_BUFFERS[1],
        ProgramBuffer::vertices("wallVertices", 0, VertexWord::Uint),
        ProgramBuffer::vertices("wallPaint", 1, VertexWord::Float).only_in(Variants::PerVertex),
        ProgramBuffer::vertices("wallHeights", 2, VertexWord::Float).only_in(Variants::Standing),
    ],
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_EXTRUSION_PROPERTIES,
    // No GL ES, whose vertex stages read no storage buffers.
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_PULLED)
        .msl(&msl::FILL_EXTRUSION_PULLED)
        .glsl(&glsl::FILL_EXTRUSION_PULLED),
};

pub static FILL_EXTRUSION_PATTERN: Program = Program {
    name: "FillExtrusionPatternShader",
    buffers: FILL_EXTRUSION_PATTERN_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        FILL_EXTRUSION_PATTERN_FROM,
        FILL_EXTRUSION_PATTERN_TO,
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: FILL_EXTRUSION_PATTERN_PROPERTIES,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_PATTERN)
        .msl(&msl::FILL_EXTRUSION_PATTERN)
        .glsl(&glsl::FILL_EXTRUSION_PATTERN)
        .glsl_es(&glsl_es::FILL_EXTRUSION_PATTERN),
};

pub static FILL_EXTRUSION_PATTERN_INSTANCED: Program = Program {
    name: "FillExtrusionPatternInstancedShader",
    buffers: FILL_EXTRUSION_PATTERN_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[
        FILL_EXTRUSION_OUTLINE_POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        FILL_EXTRUSION_PATTERN_FROM,
        FILL_EXTRUSION_PATTERN_TO,
        FILL_EXTRUSION_NEXT_OUTLINE_POS,
        FILL_EXTRUSION_NEXT_DECIMALS_ED,
        GROUND_HEIGHT,
    ],
    textures: &[IMAGE],
    properties: FILL_EXTRUSION_PATTERN_PROPERTIES,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_PATTERN_INSTANCED)
        .msl(&msl::FILL_EXTRUSION_PATTERN_INSTANCED)
        .glsl(&glsl::FILL_EXTRUSION_PATTERN_INSTANCED)
        .glsl_es(&glsl_es::FILL_EXTRUSION_PATTERN_INSTANCED),
};

/// [`FILL_EXTRUSION_WALL`] of the pattern walls.
pub static FILL_EXTRUSION_PATTERN_WALL: Program = Program {
    name: "FillExtrusionPatternWallShader",
    buffers: FILL_EXTRUSION_PATTERN_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        FILL_EXTRUSION_OUTLINE_POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        FILL_EXTRUSION_PATTERN_FROM,
        FILL_EXTRUSION_PATTERN_TO,
        FILL_EXTRUSION_NEXT_OUTLINE_POS,
        FILL_EXTRUSION_NEXT_DECIMALS_ED,
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: FILL_EXTRUSION_PATTERN_PROPERTIES,
    // As `FILL_EXTRUSION_WALL`.
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_EXTRUSION_PATTERN_INSTANCED)
        .glsl(&glsl::FILL_EXTRUSION_PATTERN_INSTANCED)
        .glsl_es(&glsl_es::FILL_EXTRUSION_PATTERN_INSTANCED),
};
