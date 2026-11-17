//! The render layers and their layer tweakers.

pub(crate) mod background_layer_tweaker;
pub(crate) mod circle_layer_tweaker;
pub(crate) mod collision_layer_tweaker;
pub(crate) mod color_relief_layer_tweaker; // dem-heatmap
pub(crate) mod fill_extrusion_layer_tweaker;
pub(crate) mod heatmap_layer_tweaker; // dem-heatmap
pub(crate) mod heatmap_texture_layer_tweaker; // dem-heatmap
pub(crate) mod hillshade_layer_tweaker; // dem-heatmap
pub(crate) mod hillshade_prepare_layer_tweaker; // dem-heatmap
pub(crate) mod line_layer_tweaker;
pub(crate) mod raster_layer_tweaker;
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
pub(crate) mod symbol_layer_tweaker;

use std::sync::Arc;

use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_style::CrossfadeParameters;
use mln_style::layer_type::ErasedLayerStyle;
use mln_style::layers::{Background, BackgroundPaintProperties};
use mln_tile_id::OverscaledTileID;
use mln_util::hash::HashMap;

use crate::draw::DrawObjects;
use crate::draw_cache::BucketChanges;
use crate::drawable::Drawable;
use crate::layer_type::{ErasedEvaluatedLayer, LayerType, evaluated};
use crate::render_source::RenderTiles;
use crate::render_tile::RenderTile;

pub(crate) fn background_layer_properties(
    properties: &Arc<dyn ErasedEvaluatedLayer>,
) -> &BackgroundPaintProperties::PossiblyEvaluated {
    &evaluated::<Background>(properties).paint
}

pub(crate) fn background_crossfade(properties: &Arc<dyn ErasedEvaluatedLayer>) -> &CrossfadeParameters {
    &evaluated::<Background>(properties).crossfade
}

/// The frame's tiles of a layer by tile id, from which its tweakers take
/// the tile and the bucket each drawable draws.
pub(crate) struct LayerTiles<'a> {
    style: Option<&'a dyn ErasedLayerStyle>,
    tiles: HashMap<OverscaledTileID, &'a RenderTile>,
}

impl<'a> LayerTiles<'a> {
    /// The tiles `render_tiles` of the layer `style` draws in the frame.
    #[must_use]
    pub(crate) fn new(style: &'a dyn ErasedLayerStyle, render_tiles: Option<&'a RenderTiles>) -> Self {
        let tiles = render_tiles.into_iter().flat_map(RenderTiles::iter);
        LayerTiles { style: Some(style), tiles: tiles.map(|tile| (*tile.get_overscaled_tile_id(), tile)).collect() }
    }

    /// No tiles, for the tweakers of drawables that draw none.
    #[must_use]
    pub(crate) fn none() -> Self {
        LayerTiles { style: None, tiles: HashMap::default() }
    }

    /// The frame's tile `tile_id`.
    #[must_use]
    pub(crate) fn tile(&self, tile_id: &OverscaledTileID) -> Option<&'a RenderTile> {
        self.tiles.get(tile_id).copied()
    }

    /// The frame's tile `tile_id` and its bucket of the layer, as a bucket of
    /// a `T` layer.
    #[must_use]
    pub(crate) fn bucket<T: LayerType>(&self, tile_id: &OverscaledTileID) -> Option<(&'a RenderTile, &'a T::Bucket)> {
        let tile = self.tile(tile_id)?;
        Some((tile, tile.get_layer_render_data(self.style?)?.bucket::<T>()?))
    }
}

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

/// Updates the drawables of a tile in `pass` of `layer_group`. `update`
/// returns true if it updated the drawable, false if it skipped it because
/// it's for a previous style. Returns true if drawables were updated.
pub(crate) fn update_tile(
    layer_group: &mut crate::layer_group::LayerGroup,
    render_pass: crate::render_pass::RenderPass,
    tile_id: &OverscaledTileID,
    mut update: impl FnMut(&mut Drawable) -> bool,
) -> bool {
    let mut any_updated = false;
    let mut un_updated_drawables = false;
    layer_group.visit_drawables_for(render_pass, tile_id, |drawable| {
        if update(drawable) {
            any_updated = true;
        } else {
            un_updated_drawables = true;
        }
    });

    // If any are updated, the caller shouldn't add new ones. If none are
    // updated and some were skipped, remove those. This is to handle the
    // case that the style layer changes but the bucket is not re-created.
    if !any_updated && un_updated_drawables {
        layer_group.remove_drawables(render_pass, tile_id);
    }

    any_updated
}
