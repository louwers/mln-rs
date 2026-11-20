//! The render layers.

pub(crate) mod render_color_relief_layer; // dem-heatmap
pub(crate) mod render_custom_layer;
pub(crate) mod render_hillshade_layer; // dem-heatmap
pub(crate) mod render_raster_layer;
pub(crate) mod render_symbol_layer;

/// The column-major `f32` matrix of `m`, as a uniform block holds it.
#[must_use]
pub fn cast_matrix(m: &glam::DMat4) -> [f32; 16] {
    m.as_mat4().to_cols_array()
}
