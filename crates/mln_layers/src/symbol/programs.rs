//! The programs of the `symbol` layer type: the attributes, textures and
//! paint properties of the shaders of its symbols and of their collision
//! boxes and circles.

use mln_gfx::gfx_types::AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UShort2, UShort4};
use mln_gfx::shaders::program::{Program, Property};
use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO,
    ID_PROJECTION_UBO,
};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_SYMBOL_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_SYMBOL_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO; // SSBO
pub const ID_SYMBOL_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const SYMBOL_LAYER_UBO_COUNT: usize = ID_SYMBOL_EVALUATED_PROPS_UBO + 1;
pub const ID_COLLISION_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub const ID_COLLISION_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_COLLISION_PROJECTION_UBO: usize = ID_PROJECTION_UBO;
pub const COLLISION_UBO_COUNT: usize = ID_COLLISION_TILE_PROPS_UBO + 1;
pub const ID_SYMBOL_IMAGE_TEXTURE: usize = 0;
pub const ID_SYMBOL_IMAGE_ICON_TEXTURE: usize = 1;
pub const ID_COLLISION_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE: usize = 1;
pub const ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE: usize = 2;
pub const ID_COLLISION_PLACED_VERTEX_ATTRIBUTE: usize = 3;
pub const ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE: usize = 4;
/// The symbol's `symbol-height-offset`.
pub const ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE: usize = 5;
pub const ID_SYMBOL_POS_OFFSET_ATTRIBUTE: usize = 0;
pub const ID_SYMBOL_DATA_ATTRIBUTE: usize = 1;
pub const ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE: usize = 2;
pub const ID_SYMBOL_PROJECTED_POS_ATTRIBUTE: usize = 3;
pub const ID_SYMBOL_FADE_OPACITY_ATTRIBUTE: usize = 4;

// Data-driven properties.
pub const ID_SYMBOL_OPACITY_ATTRIBUTE: usize = 5;
pub const ID_SYMBOL_COLOR_ATTRIBUTE: usize = 6;
pub const ID_SYMBOL_HALO_COLOR_ATTRIBUTE: usize = 7;
pub const ID_SYMBOL_HALO_WIDTH_ATTRIBUTE: usize = 8;
pub const ID_SYMBOL_HALO_BLUR_ATTRIBUTE: usize = 9;
/// The atlas origin of a quad's image.
pub const ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE: usize = 10;
/// The symbol's `symbol-height-offset`.
pub const ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE: usize = 11;

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
    textures: &[ID_SYMBOL_IMAGE_TEXTURE],
    properties: &[Property::new("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE)],
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
    textures: &[ID_SYMBOL_IMAGE_TEXTURE],
    properties: &[
        Property::new("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        Property::new("fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        Property::new("halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        Property::new("halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        Property::new("halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
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
    textures: &[ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_IMAGE_ICON_TEXTURE],
    properties: &[
        Property::new("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        Property::new("fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        Property::new("halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        Property::new("halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        Property::new("halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
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
