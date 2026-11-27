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

use crate::draw::context::Context;
use crate::draw::objects::Mesh;
use crate::draw::vertex_attribute::VertexAttributeArray;
use crate::layer::layer_drawing::BucketChanges;

/// Copies the bucket's data-driven attributes into a mesh again where
/// `changes` says their values changed: `refresh(context, attrs)` reads them
/// into its instance attributes where it has them, else into its vertex
/// attributes, which then upload again.
pub(crate) fn refresh_paint_attributes(
    mesh: &Mesh,
    context: &mut Context,
    changes: BucketChanges,
    refresh: impl FnOnce(&mut Context, &mut VertexAttributeArray),
) {
    if !changes.paint {
        return;
    }
    mesh.update(context, |objects, context| refresh(context, objects.paint_attributes_mut()));
}

#[must_use]
pub(crate) fn cast_matrix(m: &glam::DMat4) -> [f32; 16] {
    m.as_mat4().to_cols_array()
}
