//! The programs of the `heatmap` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Short2};
use mln_gfx::shaders::program::{Program, Property};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_HEATMAP_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_HEATMAP_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const HEATMAP_LAYER_UBO_COUNT: usize = ID_HEATMAP_EVALUATED_PROPS_UBO + 1;
pub const ID_HEATMAP_TEXTURE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_HEATMAP_IMAGE_TEXTURE: usize = 0;
pub const ID_HEATMAP_COLOR_RAMP_TEXTURE: usize = 1;
pub const ID_HEATMAP_POS_VERTEX_ATTRIBUTE: usize = 0;

// Data-driven properties.
pub const ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE: usize = 1;
pub const ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE: usize = 2;

pub static HEATMAP: Program = Program {
    name: "HeatmapShader",
    attributes: &[
        (ID_HEATMAP_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE, Float2),
        (ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::new("weight", ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
        Property::new("radius", ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
    ],
};

pub static HEATMAP_TEXTURE: Program = Program {
    name: "HeatmapTextureShader",
    attributes: &[(ID_HEATMAP_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_COLOR_RAMP_TEXTURE],
    properties: &[],
};
