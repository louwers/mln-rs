// The tables of the engine's shaders and the symbol layer type's describe
// the vertex layout the renderer produces: symbols that aren't instanced.
// The vertex buffers follow the shader's UBOs: the layout vertices at
// `<layer>UBOCount`, then the symbols' dynamic and opacity vertices, then the
// interleaved data-driven vertices.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::shader_defines::{
    ATMOSPHERE_LAYER_UBO_COUNT, COLLISION_UBO_COUNT, DRAWABLE_RESERVED_UBO_COUNT, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE,
    ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE,
    ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE,
    ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE,
    ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE, ID_LAYER_COMPOSITE_IMAGE_TEXTURE,
    ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE,
    ID_SKY_POS_VERTEX_ATTRIBUTE, ID_SYMBOL_COLOR_ATTRIBUTE, ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE,
    ID_SYMBOL_HALO_BLUR_ATTRIBUTE, ID_SYMBOL_HALO_COLOR_ATTRIBUTE, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE,
    ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE,
    ID_SYMBOL_OPACITY_ATTRIBUTE, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE,
    ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, LAYER_COMPOSITE_UBO_COUNT, LINE_LAYER_UBO_COUNT,
    SKY_LAYER_UBO_COUNT, SYMBOL_LAYER_UBO_COUNT,
};
use mln_gfx::shaders::source::msl::{Attribute, Source, Texture};

use super::msl_sources;

// The number of UBOs each shader binds before its vertex buffers, for the
// shaders whose count shader_defines.rs doesn't name.
const ATMOSPHERE_UBO_COUNT: usize = ATMOSPHERE_LAYER_UBO_COUNT;
const CLIPPING_MASK_UBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
const GLOBE_DEPTH_UBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
const SKY_UBO_COUNT: usize = SKY_LAYER_UBO_COUNT;
const TILE_DEBUG_UBO_COUNT: usize = LINE_LAYER_UBO_COUNT;
const SYMBOL_UBO_COUNT: usize = SYMBOL_LAYER_UBO_COUNT;

const fn attr(index: usize, data_type: AttributeDataType, buffer_index: usize, id: usize) -> Attribute {
    Attribute { index, data_type, buffer_index, id }
}

const fn tex(index: usize, id: usize) -> Texture {
    Texture { index, id }
}

use AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UByte4, UShort2, UShort4};

macro_rules! shader_source {
    ($name:ident, $attrs:expr, $textures:expr) => {
        shader_source!($name, $attrs, &[], $textures);
    };
    ($name:ident, $attrs:expr, $instance_attrs:expr, $textures:expr) => {
        static $name: Source = Source {
            msl: &msl_sources::$name,
            attributes: $attrs,
            instance_attributes: $instance_attrs,
            textures: $textures,
        };
    };
}

const CLIPPING_MASK_ATTRIBUTES: &[Attribute] =
    &[attr(0, Short2, CLIPPING_MASK_UBO_COUNT, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)];
shader_source!(CLIPPING_MASK_PROGRAM, CLIPPING_MASK_ATTRIBUTES, &[]);

const COLLISION_BOX_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short2, COLLISION_UBO_COUNT, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, COLLISION_UBO_COUNT, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    attr(2, Short2, COLLISION_UBO_COUNT, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    // Dynamic
    attr(3, UShort2, COLLISION_UBO_COUNT + 1, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
    attr(4, Float2, COLLISION_UBO_COUNT + 1, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE),
    // Layout
    attr(5, Float, COLLISION_UBO_COUNT, ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE),
];
const COLLISION_CIRCLE_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short2, COLLISION_UBO_COUNT, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, COLLISION_UBO_COUNT, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    attr(2, Short2, COLLISION_UBO_COUNT, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    // Dynamic
    attr(3, UShort2, COLLISION_UBO_COUNT + 1, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
];
shader_source!(COLLISION_BOX_SHADER, COLLISION_BOX_ATTRIBUTES, &[]);
shader_source!(COLLISION_CIRCLE_SHADER, COLLISION_CIRCLE_ATTRIBUTES, &[]);

const GLOBE_DEPTH_ATTRIBUTES: &[Attribute] =
    &[attr(0, Short2, GLOBE_DEPTH_UBO_COUNT, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE)];
shader_source!(GLOBE_DEPTH_SHADER, GLOBE_DEPTH_ATTRIBUTES, &[]);

const LAYER_COMPOSITE_ATTRIBUTES: &[Attribute] =
    &[attr(0, Short2, LAYER_COMPOSITE_UBO_COUNT, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE)];
shader_source!(
    LAYER_COMPOSITE_SHADER,
    LAYER_COMPOSITE_ATTRIBUTES,
    &[tex(0, ID_LAYER_COMPOSITE_IMAGE_TEXTURE), tex(1, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE)]
);
const TILE_DEBUG_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short2, TILE_DEBUG_UBO_COUNT, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, TILE_DEBUG_UBO_COUNT, ID_LINE_DATA_VERTEX_ATTRIBUTE),
];
shader_source!(TILE_DEBUG_SHADER, TILE_DEBUG_ATTRIBUTES, &[]);

const SKY_ATTRIBUTES: &[Attribute] = &[attr(0, Short2, SKY_UBO_COUNT, ID_SKY_POS_VERTEX_ATTRIBUTE)];
const ATMOSPHERE_ATTRIBUTES: &[Attribute] =
    &[attr(0, Short2, ATMOSPHERE_UBO_COUNT, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE)];
shader_source!(SKY_SHADER, SKY_ATTRIBUTES, &[]);
shader_source!(ATMOSPHERE_SHADER, ATMOSPHERE_ATTRIBUTES, &[]);

// The layout vertex (`SymbolLayoutVertex`), the dynamic vertex
// (`projected_pos`), the opacity vertex (`fade_opacity`) and the interleaved
// data-driven vertex, in four buffers.
const SYMBOL_ICON_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short4, SYMBOL_UBO_COUNT, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, SYMBOL_UBO_COUNT, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(2, Short4, SYMBOL_UBO_COUNT, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    // Dynamic
    attr(3, Float3, SYMBOL_UBO_COUNT + 1, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(4, Float, SYMBOL_UBO_COUNT + 2, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(5, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_OPACITY_ATTRIBUTE),
    // Layout
    attr(6, UShort2, SYMBOL_UBO_COUNT, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    attr(7, Float, SYMBOL_UBO_COUNT, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];
const SYMBOL_SDF_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short4, SYMBOL_UBO_COUNT, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, SYMBOL_UBO_COUNT, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(2, Short4, SYMBOL_UBO_COUNT, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    // Dynamic
    attr(3, Float3, SYMBOL_UBO_COUNT + 1, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(4, Float, SYMBOL_UBO_COUNT + 2, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(5, Float4, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_COLOR_ATTRIBUTE),
    attr(6, Float4, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    attr(7, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(8, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    attr(9, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    // Layout
    attr(10, UShort2, SYMBOL_UBO_COUNT, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    attr(11, Float, SYMBOL_UBO_COUNT, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];
const SYMBOL_TEXT_AND_ICON_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short4, SYMBOL_UBO_COUNT, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, SYMBOL_UBO_COUNT, ID_SYMBOL_DATA_ATTRIBUTE),
    // Dynamic
    attr(2, Float3, SYMBOL_UBO_COUNT + 1, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(3, Float, SYMBOL_UBO_COUNT + 2, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(4, Float4, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_COLOR_ATTRIBUTE),
    attr(5, Float4, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    attr(6, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(7, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    attr(8, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    // Layout
    attr(9, UShort2, SYMBOL_UBO_COUNT, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    attr(10, Float, SYMBOL_UBO_COUNT, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];
const SYMBOL_TEXTURES: &[Texture] = &[tex(0, ID_SYMBOL_IMAGE_TEXTURE)];
shader_source!(SYMBOL_ICON_SHADER, SYMBOL_ICON_ATTRIBUTES, SYMBOL_TEXTURES);
shader_source!(SYMBOL_SDF_SHADER, SYMBOL_SDF_ATTRIBUTES, SYMBOL_TEXTURES);
shader_source!(
    SYMBOL_TEXT_AND_ICON_SHADER,
    SYMBOL_TEXT_AND_ICON_ATTRIBUTES,
    &[tex(0, ID_SYMBOL_IMAGE_TEXTURE), tex(1, ID_SYMBOL_IMAGE_ICON_TEXTURE)]
);

/// The source of each of the engine's own programs, and of the symbol layer
/// type's, which is in the engine.
pub(crate) static BUILT_INS: &[(&Program, &Source)] = &[
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TILE_DEBUG, &TILE_DEBUG_SHADER),
    (&programs::SYMBOL_ICON, &SYMBOL_ICON_SHADER),
    (&programs::SYMBOL_SDF, &SYMBOL_SDF_SHADER),
    (&programs::SYMBOL_TEXT_AND_ICON, &SYMBOL_TEXT_AND_ICON_SHADER),
    (&programs::COLLISION_BOX, &COLLISION_BOX_SHADER),
    (&programs::COLLISION_CIRCLE, &COLLISION_CIRCLE_SHADER),
];
