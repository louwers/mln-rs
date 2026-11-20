//! The render layers.

pub(crate) mod render_circle_layer;
pub(crate) mod render_color_relief_layer; // dem-heatmap
pub(crate) mod render_custom_layer;
pub(crate) mod render_fill_extrusion_layer;
pub(crate) mod render_fill_layer;
pub(crate) mod render_heatmap_layer; // dem-heatmap
pub(crate) mod render_hillshade_layer; // dem-heatmap
pub(crate) mod render_line_layer;
pub(crate) mod render_raster_layer;
pub(crate) mod render_symbol_layer;

use crate::draw::DrawObjects;
use crate::gfx::vertex_attribute::VertexAttributeArray;
use crate::layer_drawing::BucketChanges;

/// Copies the bucket's data-driven attributes into a draw again where
/// `changes` says their values changed: `refresh(attrs)` reads them into its
/// instance attributes where it has them, else into its vertex attributes,
/// which then upload again.
pub(crate) fn refresh_paint_attributes(
    objects: &mut DrawObjects,
    changes: BucketChanges,
    refresh: impl FnOnce(&mut VertexAttributeArray),
) {
    if !changes.paint {
        return;
    }
    refresh(objects.paint_attributes_mut());
}

#[must_use]
pub(crate) fn cast_matrix(m: &glam::DMat4) -> [f32; 16] {
    m.as_mat4().to_cols_array()
}
