//! The render layers.

pub(crate) mod render_background_layer;
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

use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_style::layer_type::ErasedLayerStyle;

use crate::draw::DrawObjects;
use crate::draw_cache::BucketChanges;
use crate::render_tile::RenderTile;

/// Copies the bucket's data-driven attributes into a draw again where
/// `changes` says their values changed: `refresh(attrs)` reads them into its
/// instance attributes where it has them, else into its vertex attributes,
/// which then upload again.
pub(crate) fn refresh_paint_attributes(
    objects: &mut DrawObjects,
    changes: BucketChanges,
    refresh: impl FnOnce(&mut mln_gfx::vertex_attribute::VertexAttributeArray),
) {
    if !changes.paint {
        return;
    }
    if let Some(attrs) = objects.instance_attributes.as_ref().or(objects.vertex_attributes.as_ref()) {
        refresh(&mut attrs.borrow_mut());
    }
}

/// A shared gfx vertex vector holding a copy of `vertices`, for data that
/// stays in the bucket or the layer (see `bucket_geometry::released_vertices`;
/// a bucket's geometry comes from `bucket_geometry::bucket_vertices`).
pub(crate) fn shared_vertices<V: bytemuck::Pod>(vertices: &[V]) -> VertexVectorBasePtr {
    crate::bucket_geometry::released_vertices(vertices)
}

/// The tile's render data for `pass`. A free function, so that the result
/// borrows only the tile (the layers mutate themselves while they hold it).
pub(crate) fn render_data_for_pass<'a>(
    tile: &'a RenderTile,
    style: &dyn ErasedLayerStyle,
    pass: crate::render_pass::RenderPass,
) -> Option<&'a crate::render_layer::LayerRenderData> {
    let render_data = tile.get_layer_render_data(style)?;
    if (render_data.layer.render_passes() & pass.underlying_type()) != 0 { Some(render_data) } else { None }
}

#[must_use]
pub(crate) fn cast_matrix(m: &[f64; 16]) -> [f32; 16] {
    std::array::from_fn(|i| m[i] as f32)
}
