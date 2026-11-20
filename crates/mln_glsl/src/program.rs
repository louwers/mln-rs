// The tables of the engine's shaders and the symbol layer type's describe
// the vertex layout the renderer produces: symbols aren't instanced and have
// a `tex_origin` attribute.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::shader_defines::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE,
    ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE,
    ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE,
    ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE,
    ID_LAYER_COMPOSITE_BACKDROP_TEXTURE, ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE,
    ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_SKY_POS_VERTEX_ATTRIBUTE,
    ID_SYMBOL_COLOR_ATTRIBUTE, ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE,
    ID_SYMBOL_HALO_BLUR_ATTRIBUTE, ID_SYMBOL_HALO_COLOR_ATTRIBUTE, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE,
    ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE,
    ID_SYMBOL_OPACITY_ATTRIBUTE, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE,
    ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE,
};
use mln_gfx::shaders::source::glsl::{Attribute, Source, Texture};

use crate::front_end::Stage;
use crate::sources;

/// The source of `stage` of `source` for naga's GLSL front end, with
/// `defines` (`#define` lines): the version, the defines, the stage's
/// prelude, and the program's own prelude and stage.
#[must_use]
pub fn compose(source: &Source, stage: Stage, defines: &str) -> String {
    let glsl = match stage {
        Stage::Vertex => source.glsl.vertex,
        Stage::Fragment => source.glsl.fragment,
    };
    crate::front_end::compose(stage, defines, source.glsl.prelude, glsl)
}

const fn attr(index: usize, data_type: AttributeDataType, id: usize) -> Attribute {
    Attribute { index, data_type, id }
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
            glsl: &sources::$name,
            attributes: $attrs,
            instance_attributes: $instance_attrs,
            textures: $textures,
        };
    };
}

const CLIPPING_MASK_ATTRIBUTES: &[Attribute] = &[attr(0, Short2, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)];
shader_source!(CLIPPING_MASK_PROGRAM, CLIPPING_MASK_ATTRIBUTES, &[]);

const COLLISION_BOX_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short2, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    attr(2, Short2, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    // Dynamic
    attr(3, UShort2, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE),
    // Layout
    attr(5, Float, ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE),
];
const COLLISION_CIRCLE_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short2, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    attr(2, Short2, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    // Dynamic
    attr(3, UShort2, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
];
shader_source!(COLLISION_BOX_SHADER, COLLISION_BOX_ATTRIBUTES, &[]);
shader_source!(COLLISION_CIRCLE_SHADER, COLLISION_CIRCLE_ATTRIBUTES, &[]);

const GLOBE_DEPTH_ATTRIBUTES: &[Attribute] = &[attr(0, Short2, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE)];
shader_source!(GLOBE_DEPTH_SHADER, GLOBE_DEPTH_ATTRIBUTES, &[]);

const LAYER_COMPOSITE_ATTRIBUTES: &[Attribute] = &[attr(0, Short2, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE)];
shader_source!(
    LAYER_COMPOSITE_SHADER,
    LAYER_COMPOSITE_ATTRIBUTES,
    &[tex(0, ID_LAYER_COMPOSITE_IMAGE_TEXTURE), tex(1, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE)]
);
const TILE_DEBUG_ATTRIBUTES: &[Attribute] =
    &[attr(0, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE), attr(1, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE)];
shader_source!(TILE_DEBUG_SHADER, TILE_DEBUG_ATTRIBUTES, &[]);

const SKY_ATTRIBUTES: &[Attribute] = &[attr(0, Short2, ID_SKY_POS_VERTEX_ATTRIBUTE)];
const ATMOSPHERE_ATTRIBUTES: &[Attribute] = &[attr(0, Short2, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE)];
shader_source!(SKY_SHADER, SKY_ATTRIBUTES, &[]);
shader_source!(ATMOSPHERE_SHADER, ATMOSPHERE_ATTRIBUTES, &[]);

// Symbols: the layout vertex (`SymbolLayoutVertex`, with `tex_origin`), the
// dynamic vertex (`projected_pos`), the opacity vertex (`fade_opacity`) and
// the interleaved data-driven vertex, in four buffers.
const SYMBOL_ICON_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(2, Short4, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    // Dynamic
    attr(3, Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(4, Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(5, Float2, ID_SYMBOL_OPACITY_ATTRIBUTE),
    // Layout
    attr(6, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    attr(7, Float, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];
const SYMBOL_SDF_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(2, Short4, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    // Dynamic
    attr(3, Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(4, Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(5, Float4, ID_SYMBOL_COLOR_ATTRIBUTE),
    attr(6, Float4, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    attr(7, Float2, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(8, Float2, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    attr(9, Float2, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    // Layout
    attr(10, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    attr(11, Float, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];
const SYMBOL_TEXT_AND_ICON_ATTRIBUTES: &[Attribute] = &[
    attr(0, Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    // Dynamic
    attr(2, Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(3, Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(4, Float4, ID_SYMBOL_COLOR_ATTRIBUTE),
    attr(5, Float4, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    attr(6, Float2, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(7, Float2, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    attr(8, Float2, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    // Layout
    attr(9, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    attr(10, Float, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
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
pub static BUILT_INS: &[(&Program, &Source)] = &[
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
