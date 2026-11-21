//! The programs of the `line` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UByte4, UShort4};
use mln_gfx::shaders::program::{Program, Property};
use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO,
};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_LINE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_LINE_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO; // SSBO
pub const ID_LINE_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const LINE_LAYER_UBO_COUNT: usize = ID_LINE_EVALUATED_PROPS_UBO + 1;
pub const ID_LINE_IMAGE_TEXTURE: usize = 0;
pub const ID_LINE_DASH_TEXTURE: usize = 1;
pub const ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_LINE_DATA_VERTEX_ATTRIBUTE: usize = 1;

// Data-driven properties.
pub const ID_LINE_COLOR_VERTEX_ATTRIBUTE: usize = 2;
pub const ID_LINE_BLUR_VERTEX_ATTRIBUTE: usize = 3;
pub const ID_LINE_OPACITY_VERTEX_ATTRIBUTE: usize = 4;
pub const ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE: usize = 5;
pub const ID_LINE_OFFSET_VERTEX_ATTRIBUTE: usize = 6;
pub const ID_LINE_WIDTH_VERTEX_ATTRIBUTE: usize = 7;
pub const ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE: usize = 8;
pub const ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE: usize = 9;
pub const ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE: usize = 10;
pub const ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE: usize = 11;

const LINE_ATTRIBUTES: &[(usize, AttributeDataType)] = &[
    (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
    (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
    (ID_LINE_COLOR_VERTEX_ATTRIBUTE, Float4),
    (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
    (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
    (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
    (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
    (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
];

const LINE_PROPERTIES: &[Property] = &[
    Property::new("color", ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    Property::new("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    Property::new("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    Property::new("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    Property::new("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    Property::new("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];

pub static LINE: Program = Program {
    name: "LineShader",
    attributes: LINE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
    properties: LINE_PROPERTIES,
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
        Property::new("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::new("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::new("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
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
        Property::new("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::new("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::new("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("pattern_from", ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::new("pattern_to", ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE),
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
        Property::new("color", ID_LINE_COLOR_VERTEX_ATTRIBUTE),
        Property::new("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::new("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::new("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
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
        Property::new("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::new("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::new("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
    ],
};
