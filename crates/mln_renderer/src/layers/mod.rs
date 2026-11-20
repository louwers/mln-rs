//! The render layers.

pub(crate) mod render_symbol_layer;

/// The column-major `f32` matrix of `m`, as a uniform block holds it.
#[must_use]
pub fn cast_matrix(m: &glam::DMat4) -> [f32; 16] {
    m.as_mat4().to_cols_array()
}
