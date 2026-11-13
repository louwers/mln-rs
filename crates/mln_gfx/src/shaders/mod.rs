//! The backend-independent shader code: uniform buffer layouts, binding ids
//! and defines, shader groups and program bases.

pub mod background_layer_ubo;
pub mod circle_layer_ubo;
pub mod clipping_mask_ubo;
pub mod color_relief_layer_ubo;
pub mod fill_extrusion_layer_ubo;
pub mod fill_layer_ubo;
pub mod heatmap_layer_ubo;
pub mod heatmap_texture_layer_ubo;
pub mod hillshade_layer_ubo;
pub mod hillshade_prepare_layer_ubo;
pub mod layer_ubo;
pub mod line_layer_ubo;
pub mod program_parameters;
pub mod raster_layer_ubo;
pub mod segment;
pub mod shader_defines;
pub mod shader_program_base;
pub mod shader_source;
pub mod sky_ubo;
pub mod symbol_layer_ubo;
