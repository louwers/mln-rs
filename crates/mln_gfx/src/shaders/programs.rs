//! The built-in programs.

use crate::gfx_types::AttributeDataType::{Float, Float2, Float4, Short2, Short4, UByte4, UShort2, UShort4};
use crate::shaders::program::{Attribute, Program, ProgramTexture, Property};

const POS: Attribute = Attribute::new("pos", Short2);
const IMAGE: ProgramTexture = ProgramTexture::new("image");
/// Over terrain, the height of the ground under a feature's anchor, in
/// meters with the exaggeration, which the terrain variants of the programs
/// that stand on it read.
const GROUND_HEIGHT: Attribute = Attribute::new("ground_height", Float);
/// The ground's depth: each pixel's eye distance to the nearest ground drawn
/// there, the bits of an `f32`, least significant byte first, in an RGBA8
/// texel; 0 where none is. The terrain variants of the programs that hide
/// behind the ground read it, in the slot their own textures leave free.
const GROUND_DEPTH: ProgramTexture = ProgramTexture::unfilterable("ground_depth");

pub static BACKGROUND: Program =
    Program { name: "BackgroundShader", attributes: &[POS], instance_attributes: &[], textures: &[], properties: &[] };

pub static BACKGROUND_PATTERN: Program = Program {
    name: "BackgroundPatternShader",
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
};

pub static CIRCLE: Program = Program {
    name: "CircleShader",
    attributes: &[
        POS,
        Attribute::new("color", Float4),
        Attribute::new("radius", Float2),
        Attribute::new("blur", Float2),
        Attribute::new("opacity", Float2),
        Attribute::new("stroke_color", Float4),
        Attribute::new("stroke_width", Float2),
        Attribute::new("stroke_opacity", Float2),
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, GROUND_DEPTH],
    properties: &[
        Property::new("color"),
        Property::new("radius"),
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("stroke_color"),
        Property::new("stroke_width"),
        Property::new("stroke_opacity"),
    ],
};

pub static CLIPPING_MASK: Program = Program {
    name: "ClippingMaskProgram",
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

const FILL_COLOR: Attribute = Attribute::new("color", Float4);
const FILL_OPACITY: Attribute = Attribute::new("opacity", Float2);
const FILL_OUTLINE_COLOR: Attribute = Attribute::new("outline_color", Float4);
const FILL_PATTERN_FROM: Attribute = Attribute::new("pattern_from", UShort4);
const FILL_PATTERN_TO: Attribute = Attribute::new("pattern_to", UShort4);
/// An outline instance's vertex, and where its segment of the outline ends.
const FILL_OUTLINE_POS: Attribute = Attribute::new("outline_pos", Short2);
const FILL_OUTLINE_END: Attribute = Attribute::new("outline_end", Short2);
const FILL_PROPERTIES: &[Property] = &[Property::new("color"), Property::new("opacity")];
const FILL_OUTLINE_PROPERTIES: &[Property] = &[Property::new("opacity"), Property::new("outline_color")];
const FILL_PATTERN_PROPERTIES: &[Property] =
    &[Property::new("color"), Property::new("opacity"), Property::new("pattern_from"), Property::new("pattern_to")];

pub static FILL: Program = Program {
    name: "FillShader",
    attributes: &[POS, FILL_COLOR, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_PROPERTIES,
};

pub static FILL_OUTLINE: Program = Program {
    name: "FillOutlineShader",
    attributes: &[POS, FILL_OUTLINE_COLOR, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_OUTLINE_PROPERTIES,
};

pub static FILL_PATTERN: Program = Program {
    name: "FillPatternShader",
    attributes: &[POS, FILL_COLOR, FILL_PATTERN_FROM, FILL_PATTERN_TO, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: FILL_PATTERN_PROPERTIES,
};

pub static FILL_OUTLINE_PATTERN: Program = Program {
    name: "FillOutlinePatternShader",
    attributes: &[POS, FILL_COLOR, FILL_PATTERN_FROM, FILL_PATTERN_TO, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: FILL_PATTERN_PROPERTIES,
};

pub static FILL_OUTLINE_INSTANCED: Program = Program {
    name: "FillOutlineInstancedShader",
    attributes: &[POS],
    instance_attributes: &[FILL_OUTLINE_COLOR, FILL_OPACITY, FILL_OUTLINE_POS, FILL_OUTLINE_END],
    textures: &[],
    properties: FILL_OUTLINE_PROPERTIES,
};

/// The outlines of [`FILL_OUTLINE_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its segment's ends and paint values from
/// storage buffers, by its index.
pub static FILL_OUTLINE_PULLED: Program = Program {
    name: "FillOutlinePulledShader",
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_OUTLINE_PROPERTIES,
};

pub static FILL_OUTLINE_PATTERN_INSTANCED: Program = Program {
    name: "FillOutlinePatternInstancedShader",
    attributes: &[POS],
    instance_attributes: &[
        FILL_COLOR,
        FILL_PATTERN_FROM,
        FILL_PATTERN_TO,
        FILL_OPACITY,
        FILL_OUTLINE_POS,
        FILL_OUTLINE_END,
    ],
    textures: &[IMAGE],
    properties: FILL_PATTERN_PROPERTIES,
};

/// A vertex's position, doubled, with its round and up flags in the low
/// bits.
const LINE_POS_NORMAL: Attribute = Attribute::new("pos_normal", Short2);
/// A vertex's extrusion, direction and distance along the line.
const LINE_DATA: Attribute = Attribute::new("data", UByte4);
const LINE_COLOR: Attribute = Attribute::new("color", Float4);
const LINE_BLUR: Attribute = Attribute::new("blur", Float2);
const LINE_OPACITY: Attribute = Attribute::new("opacity", Float2);
const LINE_GAP_WIDTH: Attribute = Attribute::new("gapwidth", Float2);
const LINE_OFFSET: Attribute = Attribute::new("offset", Float2);
const LINE_WIDTH: Attribute = Attribute::new("width", Float2);
const LINE_FLOOR_WIDTH: Attribute = Attribute::new("floorwidth", Float2);
const LINE_PATTERN_FROM: Attribute = Attribute::new("pattern_from", UShort4);
const LINE_PATTERN_TO: Attribute = Attribute::new("pattern_to", UShort4);
/// Where a feature's dash pattern is in its tile's atlas.
const LINE_DASHARRAY: Attribute = Attribute::new("dasharray", Float4);

pub static LINE: Program = Program {
    name: "LineShader",
    attributes: &[
        LINE_POS_NORMAL,
        LINE_DATA,
        LINE_COLOR,
        LINE_BLUR,
        LINE_OPACITY,
        LINE_GAP_WIDTH,
        LINE_OFFSET,
        LINE_WIDTH,
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::new("color"),
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
    ],
};

pub static LINE_GRADIENT: Program = Program {
    name: "LineGradientShader",
    attributes: &[LINE_POS_NORMAL, LINE_DATA, LINE_BLUR, LINE_OPACITY, LINE_GAP_WIDTH, LINE_OFFSET, LINE_WIDTH],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
    ],
};

pub static LINE_PATTERN: Program = Program {
    name: "LinePatternShader",
    attributes: &[
        LINE_POS_NORMAL,
        LINE_DATA,
        LINE_BLUR,
        LINE_OPACITY,
        LINE_GAP_WIDTH,
        LINE_OFFSET,
        LINE_WIDTH,
        LINE_PATTERN_FROM,
        LINE_PATTERN_TO,
    ],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
        Property::new("pattern_from"),
        Property::new("pattern_to"),
    ],
};

pub static LINE_SDF: Program = Program {
    name: "LineSDFShader",
    attributes: &[
        LINE_POS_NORMAL,
        LINE_DATA,
        LINE_COLOR,
        LINE_BLUR,
        LINE_OPACITY,
        LINE_GAP_WIDTH,
        LINE_OFFSET,
        LINE_WIDTH,
        LINE_FLOOR_WIDTH,
        LINE_DASHARRAY,
    ],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[
        Property::new("color"),
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
        Property::new("floorwidth"),
        Property::new("dasharray"),
    ],
};

pub static LINE_GRADIENT_SDF: Program = Program {
    name: "LineGradientSDFShader",
    attributes: &[
        LINE_POS_NORMAL,
        LINE_DATA,
        LINE_BLUR,
        LINE_OPACITY,
        LINE_GAP_WIDTH,
        LINE_OFFSET,
        LINE_WIDTH,
        LINE_FLOOR_WIDTH,
        LINE_DASHARRAY,
    ],
    instance_attributes: &[],
    textures: &[IMAGE, ProgramTexture::new("image_dash")],
    properties: &[
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
        Property::new("floorwidth"),
        Property::new("dasharray"),
    ],
};

/// Where a vertex samples the program's textures, in their texels.
const TEXTURE_POS: Attribute = Attribute::new("texture_pos", Short2);

pub static RASTER: Program = Program {
    name: "RasterShader",
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[ProgramTexture::new("image0"), ProgramTexture::new("image1")],
    properties: &[],
};

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
};

pub static FILL_EXTRUSION_INSTANCED: Program = Program {
    name: "FillExtrusionInstancedShader",
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
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] with every attribute per
/// vertex, for walls whose quads are in the vertex buffer.
pub static FILL_EXTRUSION_WALL: Program = Program {
    name: "FillExtrusionWallShader",
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
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its wall's outline vertices and paint values
/// from storage buffers, by its index.
pub static FILL_EXTRUSION_PULLED: Program = Program {
    name: "FillExtrusionPulledShader",
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_EXTRUSION_PROPERTIES,
};

pub static FILL_EXTRUSION_PATTERN: Program = Program {
    name: "FillExtrusionPatternShader",
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
};

pub static FILL_EXTRUSION_PATTERN_INSTANCED: Program = Program {
    name: "FillExtrusionPatternInstancedShader",
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
};

/// [`FILL_EXTRUSION_WALL`] of the pattern walls.
pub static FILL_EXTRUSION_PATTERN_WALL: Program = Program {
    name: "FillExtrusionPatternWallShader",
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
};

pub static HEATMAP: Program = Program {
    name: "HeatmapShader",
    attributes: &[POS, Attribute::new("weight", Float2), Attribute::new("radius", Float2), GROUND_HEIGHT],
    instance_attributes: &[],
    textures: &[],
    properties: &[Property::new("weight"), Property::new("radius")],
};

pub static HEATMAP_TEXTURE: Program = Program {
    name: "HeatmapTextureShader",
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[IMAGE, ProgramTexture::new("color_ramp")],
    properties: &[],
};

pub static HILLSHADE: Program = Program {
    name: "HillshadeShader",
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
};

pub static HILLSHADE_PREPARE: Program = Program {
    name: "HillshadePrepareShader",
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
};

pub static COLOR_RELIEF: Program = Program {
    name: "ColorReliefShader",
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[IMAGE, ProgramTexture::unfilterable("elevation_stops"), ProgramTexture::new("color_stops")],
    properties: &[],
};

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
const SYMBOL_TEXTURE: ProgramTexture = ProgramTexture::new("texture");

pub static SYMBOL_ICON: Program = Program {
    name: "SymbolIconShader",
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
};

pub static SYMBOL_SDF: Program = Program {
    name: "SymbolSDFShader",
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
};

pub static SYMBOL_TEXT_AND_ICON: Program = Program {
    name: "SymbolTextAndIconShader",
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
    textures: &[SYMBOL_TEXTURE, ProgramTexture::new("texture_icon"), GROUND_DEPTH],
    properties: SYMBOL_SDF_PROPERTIES,
};

const COLLISION_ANCHOR_POS: Attribute = Attribute::new("anchor_pos", Short2);
const COLLISION_EXTRUDE: Attribute = Attribute::new("extrude", Short2);
const COLLISION_PLACED: Attribute = Attribute::new("placed", UShort2);

pub static COLLISION_BOX: Program = Program {
    name: "CollisionBoxShader",
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
};

pub static COLLISION_CIRCLE: Program = Program {
    name: "CollisionCircleShader",
    attributes: &[POS, COLLISION_ANCHOR_POS, COLLISION_EXTRUDE, COLLISION_PLACED],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static GLOBE_DEPTH: Program =
    Program { name: "GlobeDepthShader", attributes: &[POS], instance_attributes: &[], textures: &[], properties: &[] };

pub static SKY: Program =
    Program { name: "SkyShader", attributes: &[POS], instance_attributes: &[], textures: &[], properties: &[] };

pub static ATMOSPHERE: Program =
    Program { name: "AtmosphereShader", attributes: &[POS], instance_attributes: &[], textures: &[], properties: &[] };

/// A layer drawn into a texture, composited onto the frame.
pub static LAYER_COMPOSITE: Program = Program {
    name: "LayerCompositeShader",
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[IMAGE, ProgramTexture::new("backdrop")],
    properties: &[],
};

/// The DEM: RGBA8 texels behind a one-texel border, which the vertex stage
/// reads texel by texel, without a sampler. It is at slot 2, which the
/// sources name, so that a program that reads the ground keeps slots 0 and 1
/// for its own textures.
const GROUND_DEM: ProgramTexture = ProgramTexture::unfilterable("ground_dem");

/// The terrain's surface: a grid over a tile, raised by its DEM, with the
/// tile's drape over it, which the fragment stage samples.
pub static TERRAIN: Program = Program {
    name: "TerrainShader",
    // A grid vertex: x and y in tile units, whether it is a skirt's lower
    // edge, and 0.
    attributes: &[Attribute::new("pos", Short4)],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, ProgramTexture::new("drape"), GROUND_DEM],
    properties: &[],
};

/// The programs with variants that stand on terrain
/// ([`ProjectionVariant::stands`](crate::projection_variant::ProjectionVariant::stands)).
pub static STANDING: &[&Program] = &[
    &CIRCLE,
    &HEATMAP,
    &FILL_EXTRUSION,
    &FILL_EXTRUSION_INSTANCED,
    &FILL_EXTRUSION_WALL,
    &FILL_EXTRUSION_PULLED,
    &FILL_EXTRUSION_PATTERN,
    &FILL_EXTRUSION_PATTERN_INSTANCED,
    &FILL_EXTRUSION_PATTERN_WALL,
    &SYMBOL_ICON,
    &SYMBOL_SDF,
    &SYMBOL_TEXT_AND_ICON,
];

/// The ground's depth: the terrain's surface, as [`TERRAIN`] raises it,
/// writing each pixel's eye distance, which the layers that stand on
/// terrain read to hide behind the ground in front of them.
pub static TERRAIN_DEPTH: Program = Program {
    name: "TerrainDepthShader",
    attributes: &[Attribute::new("pos", Short4)],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, ProgramTexture::UNUSED, GROUND_DEM],
    properties: &[],
};

/// For the backend conformance cases: quads whose vertex stage reads their
/// color from a texture, by texel. The renderer doesn't draw with it.
#[cfg(feature = "gpu-conformance")]
pub static VERTEX_TEXEL: Program = Program {
    name: "VertexTexelShader",
    attributes: &[Attribute::new("pos_texel", Short4)],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
};

/// For the backend conformance cases: probes whose vertex stage reads a
/// height by the terrain's height rule, and whose fragment stage writes its
/// bits. The renderer doesn't draw with it.
#[cfg(feature = "gpu-conformance")]
pub static TERRAIN_PROBE: Program = Program {
    name: "TerrainProbeShader",
    attributes: &[
        // Where a probe draws, in clip x and y, then where it reads the
        // height, in tile units.
        Attribute::new("pos", Float4),
        // Where the probe reads its DEM: the scale and offset that map the
        // tile's uv to the DEM's (`uv * scale + offset`), then the DEM's
        // width in pixels, 0 for flat ground.
        Attribute::new("dem_coords", Float4),
    ],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, ProgramTexture::UNUSED, GROUND_DEM],
    properties: &[],
};
