//! The render layers, and each built-in layer type's programs, the blocks
//! they read and their text in each language.

pub mod background;
pub mod circle;
pub mod color_relief;
pub mod custom;
pub mod fill;
pub mod fill_extrusion;
pub mod heatmap;
pub mod hillshade;
pub mod line;
mod pattern;
pub mod raster;

pub(crate) mod render_background_layer;
pub(crate) mod render_circle_layer;
pub(crate) mod render_color_relief_layer; // dem-heatmap
pub(crate) mod render_fill_extrusion_layer;
pub(crate) mod render_fill_layer;
pub(crate) mod render_heatmap_layer; // dem-heatmap
pub(crate) mod render_hillshade_layer; // dem-heatmap
pub(crate) mod render_line_layer;
pub(crate) mod render_raster_layer;

use crate::draw::objects::DrawObjects;
use crate::draw::vertex_attribute::VertexAttributeArray;
use crate::layer::bucket::UploadNeeds;
use crate::layer::layer_drawing::BucketChanges;

/// What the layers' draws read of the buckets on the device of `context`.
pub(crate) fn upload_needs(context: &crate::draw::context::Context) -> UploadNeeds {
    UploadNeeds {
        fill_outline_ends: render_fill_layer::draws_outline_quads(context),
        wall_quads: !render_fill_extrusion_layer::instanced_walls(context),
    }
}

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
