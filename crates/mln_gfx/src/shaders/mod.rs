//! The backend-independent shader code: uniform blocks, binding ids and
//! defines, and what each program reads.

pub mod background_layer_ubo;
pub mod bindings;
pub mod block;
pub mod circle_layer_ubo;
pub mod color_relief_layer_ubo;
pub mod fill_extrusion_layer_ubo;
pub mod fill_layer_ubo;
pub mod heatmap_layer_ubo;
pub mod heatmap_texture_layer_ubo;
pub mod hillshade_layer_ubo;
pub mod hillshade_prepare_layer_ubo;
pub mod layer_composite_ubo;
pub mod layer_ubo;
pub mod line_layer_ubo;
pub mod program;
pub mod program_parameters;
pub mod programs;
pub mod raster_layer_ubo;
pub mod segment;
pub mod sky_ubo;
pub mod symbol_layer_ubo;
pub mod terrain_ubo;
