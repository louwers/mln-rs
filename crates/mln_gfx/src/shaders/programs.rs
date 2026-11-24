//! The built-in programs.

use crate::gfx_types::AttributeDataType;
use crate::gfx_types::AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UByte4, UShort2, UShort4};
use crate::projection_variant::ProjectionVariant;
use crate::shaders::program::{Program, Property};
use crate::shaders::shader_defines::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_IMAGE_TEXTURE, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE,
    ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE, ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE,
    ID_CIRCLE_POS_VERTEX_ATTRIBUTE, ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE, ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE,
    ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE, ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE,
    ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE,
    ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE,
    ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE,
    ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE, ID_COLOR_RELIEF_IMAGE_TEXTURE,
    ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_FILL_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_CENTROID_ATTRIBUTE,
    ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE,
    ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_IMAGE_TEXTURE,
    ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, ID_FILL_IMAGE_TEXTURE,
    ID_FILL_OPACITY_VERTEX_ATTRIBUTE, ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, ID_FILL_OUTLINE_END_ATTRIBUTE,
    ID_FILL_OUTLINE_POS_ATTRIBUTE, ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_FILL_POS_VERTEX_ATTRIBUTE, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_GROUND_DEM_TEXTURE, ID_GROUND_DEPTH_TEXTURE,
    ID_HEATMAP_COLOR_RAMP_TEXTURE, ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_POS_VERTEX_ATTRIBUTE,
    ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE, ID_HILLSHADE_IMAGE_TEXTURE,
    ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE,
    ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, ID_LINE_BLUR_VERTEX_ATTRIBUTE,
    ID_LINE_COLOR_VERTEX_ATTRIBUTE, ID_LINE_DASH_TEXTURE, ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE,
    ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE,
    ID_LINE_IMAGE_TEXTURE, ID_LINE_OFFSET_VERTEX_ATTRIBUTE, ID_LINE_OPACITY_VERTEX_ATTRIBUTE,
    ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE,
    ID_LINE_WIDTH_VERTEX_ATTRIBUTE, ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE,
    ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_SKY_POS_VERTEX_ATTRIBUTE, ID_SYMBOL_COLOR_ATTRIBUTE,
    ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, ID_SYMBOL_HALO_BLUR_ATTRIBUTE,
    ID_SYMBOL_HALO_COLOR_ATTRIBUTE, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE,
    ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_OPACITY_ATTRIBUTE,
    ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE,
    ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, ID_TERRAIN_DEM_TEXTURE, ID_TERRAIN_DRAPE_TEXTURE, ID_TERRAIN_POS_VERTEX_ATTRIBUTE,
};
#[cfg(feature = "gpu-conformance")]
use crate::shaders::shader_defines::{
    ID_TERRAIN_PROBE_DEM_COORDS_VERTEX_ATTRIBUTE, ID_TERRAIN_PROBE_POS_VERTEX_ATTRIBUTE,
    ID_VERTEX_TEXEL_POS_TEXEL_VERTEX_ATTRIBUTE, ID_VERTEX_TEXEL_TEXTURE,
};

/// The projection variants of `program`: every program's Mercator and globe
/// variants, and the terrain variants of a standing layer's program, on
/// Mercator and on the globe, which read the ground
/// ([`ID_GROUND_DEM_TEXTURE`]).
#[must_use]
pub fn projection_variants(program: &Program) -> &'static [ProjectionVariant] {
    const FLAT: &[ProjectionVariant] = &[ProjectionVariant::Mercator, ProjectionVariant::Globe];
    const STANDING: &[ProjectionVariant] = &[
        ProjectionVariant::Mercator,
        ProjectionVariant::Globe,
        ProjectionVariant::Terrain,
        ProjectionVariant::GlobeTerrain,
    ];
    const STANDING_PROGRAMS: [&Program; 12] = [
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
    if STANDING_PROGRAMS.contains(&program) { STANDING } else { FLAT }
}

/// Attributes, each id with the format a draw without data for it binds.
type Attributes<const N: usize> = [(usize, AttributeDataType); N];

/// A program variant's attributes, as [`Program::attributes`] gives them:
/// those it reads per vertex, then those it reads per instance.
pub type VariantAttributes = (&'static [(usize, AttributeDataType)], &'static [(usize, AttributeDataType)]);

/// `attributes` and then the centroid of the polygon that each vertex or
/// wall instance stands on over terrain, which the terrain variants of the
/// fill extrusion programs read. Takes `M`, the count, as one more than `N`.
const fn with_centroid<const N: usize, const M: usize>(attributes: Attributes<N>) -> Attributes<M> {
    assert!(M == N + 1);
    let mut all = [(ID_FILL_EXTRUSION_CENTROID_ATTRIBUTE, Short2); M];
    let mut i = 0;
    while i < N {
        all[i] = attributes[i];
        i += 1;
    }
    all
}

/// The attributes of `program`'s terrain variant, by vertex and by
/// instance, where they aren't the program's: the fill extrusion programs'
/// read the centroid that a roof vertex or a wall stands on
/// ([`ID_FILL_EXTRUSION_CENTROID_ATTRIBUTE`]), where they read attributes.
/// Only a terrain variant's draws bind it.
#[must_use]
pub fn terrain_attributes(program: &Program) -> Option<VariantAttributes> {
    static TERRAIN: [(&Program, VariantAttributes); 6] = [
        (&FILL_EXTRUSION, (&with_centroid::<5, 6>(FILL_EXTRUSION_ATTRIBUTES), &[])),
        (
            &FILL_EXTRUSION_INSTANCED,
            (&FILL_EXTRUSION_INSTANCED_ATTRIBUTES, &with_centroid::<7, 8>(FILL_EXTRUSION_INSTANCE_ATTRIBUTES)),
        ),
        (&FILL_EXTRUSION_WALL, (&with_centroid::<8, 9>(FILL_EXTRUSION_WALL_ATTRIBUTES), &[])),
        (&FILL_EXTRUSION_PATTERN, (&with_centroid::<6, 7>(FILL_EXTRUSION_PATTERN_ATTRIBUTES), &[])),
        (
            &FILL_EXTRUSION_PATTERN_INSTANCED,
            (&FILL_EXTRUSION_INSTANCED_ATTRIBUTES, &with_centroid::<8, 9>(FILL_EXTRUSION_PATTERN_INSTANCE_ATTRIBUTES)),
        ),
        (&FILL_EXTRUSION_PATTERN_WALL, (&with_centroid::<9, 10>(FILL_EXTRUSION_PATTERN_WALL_ATTRIBUTES), &[])),
    ];
    TERRAIN.iter().find(|(terrain, _)| *terrain == program).map(|&(_, attributes)| attributes)
}

const fn property(name: &'static str, attribute: usize) -> Property {
    Property { name, attribute }
}

pub static BACKGROUND: Program = Program {
    name: "BackgroundShader",
    attributes: &[(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static BACKGROUND_PATTERN: Program = Program {
    name: "BackgroundPatternShader",
    attributes: &[(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_BACKGROUND_IMAGE_TEXTURE],
    properties: &[],
};

pub static CIRCLE: Program = Program {
    name: "CircleShader",
    attributes: &[
        (ID_CIRCLE_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    // The terrain variant's ground, and the depth of the ground that hides
    // circles behind it.
    textures: &[ID_GROUND_DEM_TEXTURE, ID_GROUND_DEPTH_TEXTURE],
    properties: &[
        property("color", ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE),
        property("radius", ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE),
        property("blur", ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE),
        property("stroke_color", ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE),
        property("stroke_width", ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE),
        property("stroke_opacity", ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE),
    ],
};

pub static CLIPPING_MASK: Program = Program {
    name: "ClippingMaskProgram",
    attributes: &[(ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static FILL: Program = Program {
    name: "FillShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE: Program = Program {
    name: "FillOutlineShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_PATTERN: Program = Program {
    name: "FillPatternShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[ID_FILL_IMAGE_TEXTURE],
    properties: &[
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE_PATTERN: Program = Program {
    name: "FillOutlinePatternShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[ID_FILL_IMAGE_TEXTURE],
    properties: &[
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE_INSTANCED: Program = Program {
    name: "FillOutlineInstancedShader",
    attributes: &[(ID_FILL_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[
        (ID_FILL_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_END_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    textures: &[],
    properties: &[
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
};

/// The outlines of [`FILL_OUTLINE_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its segment's ends and paint values from
/// storage buffers, by its index.
pub static FILL_OUTLINE_PULLED: Program = Program {
    name: "FillOutlinePulledShader",
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE_PATTERN_INSTANCED: Program = Program {
    name: "FillOutlinePatternInstancedShader",
    attributes: &[(ID_FILL_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[
        (ID_FILL_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_END_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    textures: &[ID_FILL_IMAGE_TEXTURE],
    properties: &[
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static LINE: Program = Program {
    name: "LineShader",
    attributes: &[
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
        (ID_LINE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("color", ID_LINE_COLOR_VERTEX_ATTRIBUTE),
        property("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        property("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        property("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        property("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    ],
};

pub static LINE_GRADIENT: Program = Program {
    name: "LineGradientShader",
    attributes: &[
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
        (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[ID_LINE_IMAGE_TEXTURE],
    properties: &[
        property("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        property("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        property("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        property("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    ],
};

pub static LINE_PATTERN: Program = Program {
    name: "LinePatternShader",
    attributes: &[
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
        (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
    ],
    instance_attributes: &[],
    textures: &[ID_LINE_IMAGE_TEXTURE],
    properties: &[
        property("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        property("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        property("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        property("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static LINE_SDF: Program = Program {
    name: "LineSDFShader",
    attributes: &[
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
        (ID_LINE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, Float4),
    ],
    instance_attributes: &[],
    textures: &[ID_LINE_IMAGE_TEXTURE],
    properties: &[
        property("color", ID_LINE_COLOR_VERTEX_ATTRIBUTE),
        property("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        property("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        property("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        property("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        property("floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        property("dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
    ],
};

pub static LINE_GRADIENT_SDF: Program = Program {
    name: "LineGradientSDFShader",
    attributes: &[
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
        (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, Float4),
    ],
    instance_attributes: &[],
    textures: &[ID_LINE_IMAGE_TEXTURE, ID_LINE_DASH_TEXTURE],
    properties: &[
        property("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        property("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        property("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        property("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        property("floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        property("dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
    ],
};

pub static RASTER: Program = Program {
    name: "RasterShader",
    attributes: &[(ID_RASTER_POS_VERTEX_ATTRIBUTE, Short2), (ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE],
    properties: &[],
};

const FILL_EXTRUSION_ATTRIBUTES: Attributes<5> = [
    (ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
    (ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, Float4),
    (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
    (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
];
const FILL_EXTRUSION_INSTANCED_ATTRIBUTES: Attributes<1> = [(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2)];
const FILL_EXTRUSION_INSTANCE_ATTRIBUTES: Attributes<7> = [
    (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
    (ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, UShort2),
    (ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, Float4),
    (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
    (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
];
const FILL_EXTRUSION_WALL_ATTRIBUTES: Attributes<8> = [
    (ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
    (ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, UShort2),
    (ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, Float4),
    (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
    (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
];
const FILL_EXTRUSION_PATTERN_ATTRIBUTES: Attributes<6> = [
    (ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
    (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
    (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
    (ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
    (ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
];
const FILL_EXTRUSION_PATTERN_INSTANCE_ATTRIBUTES: Attributes<8> = [
    (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
    (ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, UShort2),
    (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
    (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
    (ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
    (ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
];
const FILL_EXTRUSION_PATTERN_WALL_ATTRIBUTES: Attributes<9> = [
    (ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
    (ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, Short2),
    (ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, UShort2),
    (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
    (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
    (ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
    (ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
];

pub static FILL_EXTRUSION: Program = Program {
    name: "FillExtrusionShader",
    attributes: &FILL_EXTRUSION_ATTRIBUTES,
    instance_attributes: &[],
    // The terrain variant's ground.
    textures: &[ID_GROUND_DEM_TEXTURE],
    properties: &[
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_EXTRUSION_INSTANCED: Program = Program {
    name: "FillExtrusionInstancedShader",
    attributes: &FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    instance_attributes: &FILL_EXTRUSION_INSTANCE_ATTRIBUTES,
    textures: &[ID_GROUND_DEM_TEXTURE],
    properties: &[
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] with every attribute per
/// vertex, for walls whose quads are in the vertex buffer.
pub static FILL_EXTRUSION_WALL: Program = Program {
    name: "FillExtrusionWallShader",
    attributes: &FILL_EXTRUSION_WALL_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[ID_GROUND_DEM_TEXTURE],
    properties: &[
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its wall's outline vertices and paint values
/// from storage buffers, by its index.
pub static FILL_EXTRUSION_PULLED: Program = Program {
    name: "FillExtrusionPulledShader",
    attributes: &[],
    instance_attributes: &[],
    textures: &[ID_GROUND_DEM_TEXTURE],
    properties: &[
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_EXTRUSION_PATTERN: Program = Program {
    name: "FillExtrusionPatternShader",
    attributes: &FILL_EXTRUSION_PATTERN_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_GROUND_DEM_TEXTURE],
    properties: &[
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_EXTRUSION_PATTERN_INSTANCED: Program = Program {
    name: "FillExtrusionPatternInstancedShader",
    attributes: &FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    instance_attributes: &FILL_EXTRUSION_PATTERN_INSTANCE_ATTRIBUTES,
    textures: &[ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_GROUND_DEM_TEXTURE],
    properties: &[
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

/// [`FILL_EXTRUSION_WALL`] of the pattern walls.
pub static FILL_EXTRUSION_PATTERN_WALL: Program = Program {
    name: "FillExtrusionPatternWallShader",
    attributes: &FILL_EXTRUSION_PATTERN_WALL_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_GROUND_DEM_TEXTURE],
    properties: &[
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static HEATMAP: Program = Program {
    name: "HeatmapShader",
    attributes: &[
        (ID_HEATMAP_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE, Float2),
        (ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    // The terrain variant's ground.
    textures: &[ID_GROUND_DEM_TEXTURE],
    properties: &[
        property("weight", ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
        property("radius", ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
    ],
};

pub static HEATMAP_TEXTURE: Program = Program {
    name: "HeatmapTextureShader",
    attributes: &[(ID_HEATMAP_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_COLOR_RAMP_TEXTURE],
    properties: &[],
};

pub static HILLSHADE: Program = Program {
    name: "HillshadeShader",
    attributes: &[(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, Short2), (ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HILLSHADE_IMAGE_TEXTURE],
    properties: &[],
};

/// A hillshade's tiles as their slopes, as [`HILLSHADE_PREPARE`] prepared
/// them, unlit: what a hillshade drapes over terrain where the surface
/// lights it ([`TERRAIN_HILLSHADE`]).
pub static HILLSHADE_SLOPES: Program = Program {
    name: "HillshadeSlopesShader",
    attributes: &[(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, Short2), (ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HILLSHADE_IMAGE_TEXTURE],
    properties: &[],
};

pub static HILLSHADE_PREPARE: Program = Program {
    name: "HillshadePrepareShader",
    attributes: &[(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, Short2), (ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HILLSHADE_IMAGE_TEXTURE],
    properties: &[],
};

pub static COLOR_RELIEF: Program = Program {
    name: "ColorReliefShader",
    attributes: &[
        (ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2),
    ],
    instance_attributes: &[],
    textures: &[
        ID_COLOR_RELIEF_IMAGE_TEXTURE,
        ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE,
        ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE,
    ],
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
    textures: &[ID_SYMBOL_IMAGE_TEXTURE, ID_GROUND_DEM_TEXTURE, ID_GROUND_DEPTH_TEXTURE],
    properties: &[property("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE)],
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
    textures: &[ID_SYMBOL_IMAGE_TEXTURE, ID_GROUND_DEM_TEXTURE, ID_GROUND_DEPTH_TEXTURE],
    properties: &[
        property("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        property("fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        property("halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        property("halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        property("halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
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
    textures: &[ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_GROUND_DEM_TEXTURE, ID_GROUND_DEPTH_TEXTURE],
    properties: &[
        property("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        property("fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        property("halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        property("halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        property("halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
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

/// The terrain's surface: a grid over a tile, raised by its DEM, with the
/// tile's drape over it.
pub static TERRAIN: Program = Program {
    name: "TerrainShader",
    attributes: &[(ID_TERRAIN_POS_VERTEX_ATTRIBUTE, Short4)],
    instance_attributes: &[],
    textures: &[ID_TERRAIN_DEM_TEXTURE, ID_TERRAIN_DRAPE_TEXTURE],
    properties: &[],
};

/// The terrain's surface with a drape of a hillshade's slopes over it
/// ([`HILLSHADE_SLOPES`]), which it lights as [`HILLSHADE`] does, with the
/// lights of the frame.
pub static TERRAIN_HILLSHADE: Program = Program {
    name: "TerrainHillshadeShader",
    attributes: &[(ID_TERRAIN_POS_VERTEX_ATTRIBUTE, Short4)],
    instance_attributes: &[],
    textures: &[ID_TERRAIN_DEM_TEXTURE, ID_TERRAIN_DRAPE_TEXTURE],
    properties: &[],
};

/// The depth of the terrain's surface, which the standing layers that it
/// hides read: a tile's grid with its skirts, raised by its DEM as the
/// surface's, whose fragments write the eye distance's (the clip w's) bits,
/// least significant byte first, as a texel's channels.
pub static TERRAIN_DEPTH: Program = Program {
    name: "TerrainDepthShader",
    attributes: &[(ID_TERRAIN_POS_VERTEX_ATTRIBUTE, Short4)],
    instance_attributes: &[],
    textures: &[ID_TERRAIN_DEM_TEXTURE],
    properties: &[],
};

/// For the backend conformance cases: quads whose vertex stage reads their
/// color from a texture, by texel. The renderer doesn't draw with it.
#[cfg(feature = "gpu-conformance")]
pub static VERTEX_TEXEL: Program = Program {
    name: "VertexTexelShader",
    attributes: &[(ID_VERTEX_TEXEL_POS_TEXEL_VERTEX_ATTRIBUTE, Short4)],
    instance_attributes: &[],
    textures: &[ID_VERTEX_TEXEL_TEXTURE],
    properties: &[],
};

/// For the backend conformance cases: probes whose vertex stage reads a
/// height by the terrain's height rule, and whose fragment stage writes its
/// bits. The renderer doesn't draw with it.
#[cfg(feature = "gpu-conformance")]
pub static TERRAIN_PROBE: Program = Program {
    name: "TerrainProbeShader",
    attributes: &[
        (ID_TERRAIN_PROBE_POS_VERTEX_ATTRIBUTE, Float4),
        (ID_TERRAIN_PROBE_DEM_COORDS_VERTEX_ATTRIBUTE, Float4),
    ],
    instance_attributes: &[],
    textures: &[ID_TERRAIN_DEM_TEXTURE],
    properties: &[],
};
