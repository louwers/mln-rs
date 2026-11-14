//! The render layers and their layer tweakers.

pub(crate) mod background_layer_tweaker;
pub(crate) mod circle_layer_tweaker;
pub(crate) mod collision_layer_tweaker;
pub(crate) mod color_relief_layer_tweaker; // dem-heatmap
pub(crate) mod fill_extrusion_layer_tweaker;
pub(crate) mod fill_layer_tweaker;
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

use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_style::CrossfadeParameters;
use mln_style::layer_properties::{LayerProperties, get_crossfade, get_evaluated};
use mln_style::layers::{BackgroundLayerProperties, BackgroundPaintProperties};
pub(crate) use render_background_layer::RenderBackgroundLayer;
pub(crate) use render_circle_layer::RenderCircleLayer;
pub(crate) use render_fill_extrusion_layer::RenderFillExtrusionLayer;
pub(crate) use render_fill_layer::RenderFillLayer;
pub(crate) use render_line_layer::RenderLineLayer;
pub(crate) use render_raster_layer::RenderRasterLayer;
pub(crate) use render_symbol_layer::RenderSymbolLayer;

use crate::bucket::{Bucket, BucketPtr};
use crate::drawable::Drawable;
use crate::layer_tweaker::LayerTweakerPtr;
use crate::render_tile::RenderTile;

pub(crate) fn background_layer_properties(
    properties: &Arc<dyn LayerProperties>,
) -> &BackgroundPaintProperties::PossiblyEvaluated {
    get_evaluated::<BackgroundLayerProperties>(properties)
}

pub(crate) fn background_crossfade(properties: &Arc<dyn LayerProperties>) -> &CrossfadeParameters {
    get_crossfade::<BackgroundLayerProperties>(properties)
}

/// The bucket a drawable's data comes from, kept alive by the drawable, and
/// the id of the layer whose binders in it the drawable uses.
pub(crate) struct DrawableBinders {
    pub(crate) bucket: BucketPtr,
    pub(crate) layer_id: String,
    /// The bucket's version when the drawable's attribute data was copied;
    /// see `refresh_if_bucket_updated`.
    pub(crate) version: std::cell::Cell<u64>,
}

impl DrawableBinders {
    #[must_use]
    pub(crate) fn new(bucket: &BucketPtr, layer_id: &str) -> Rc<Self> {
        Rc::new(DrawableBinders {
            bucket: Arc::clone(bucket),
            layer_id: layer_id.to_owned(),
            version: std::cell::Cell::new(bucket.get_version()),
        })
    }
}

/// Refreshes the drawable's copies of the bucket's data-driven attributes
/// when the bucket's version moved since the copy: `refresh(attrs)` re-reads
/// them into the drawable's attribute array, and new attributes are uploaded
/// again.
pub(crate) fn refresh_if_bucket_updated(
    drawable: &mut Drawable,
    refresh: impl FnOnce(&mut mln_gfx::vertex_attribute::VertexAttributeArray),
) {
    let Some(binders) = &drawable.binders else {
        return;
    };
    let version = binders.bucket.get_version();
    if binders.version.get() == version {
        return;
    }
    binders.version.set(version);
    if let Some(attrs) = &drawable.vertex_attributes {
        refresh(&mut attrs.borrow_mut());
    }
}

/// The bucket of `binders`, as the type the layer created, and the id of the
/// layer.
pub(crate) fn slot_bucket<B: Bucket>(binders: &DrawableBinders) -> Option<(&B, &str)> {
    let bucket = (*binders.bucket).downcast_ref::<B>()?;
    Some((bucket, binders.layer_id.as_str()))
}

/// The tile a drawable draws: the shared tiles of the frame, and the tile's
/// index in them.
pub(crate) struct DrawableRenderTile {
    pub(crate) owner: Rc<Vec<RenderTile>>,
    pub(crate) index: usize,
}

impl DrawableRenderTile {
    /// `tile` of `owner`, or `None` where `owner` doesn't hold it.
    #[must_use]
    pub(crate) fn new(owner: &Rc<Vec<RenderTile>>, tile: &RenderTile) -> Option<Self> {
        let index = owner.iter().position(|t| std::ptr::eq(t, tile))?;
        Some(DrawableRenderTile { owner: Rc::clone(owner), index })
    }

    #[must_use]
    pub(crate) fn tile(&self) -> &RenderTile {
        &self.owner[self.index]
    }
}

pub(crate) fn drawable_has_layer_tweaker(drawable: &Drawable, layer_tweaker: Option<&LayerTweakerPtr>) -> bool {
    match (&drawable.layer_tweaker, layer_tweaker) {
        (None, None) => true,
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

/// A shared gfx vertex vector holding a copy of `vertices`, for data that
/// stays in the bucket or the layer (see `bucket_geometry::released_vertices`;
/// a bucket's geometry comes from `bucket_geometry::bucket_vertices`).
pub(crate) fn shared_vertices<V: bytemuck::Pod>(vertices: &[V]) -> VertexVectorBasePtr {
    crate::bucket_geometry::released_vertices(vertices)
}

/// Whether the drawable references the tweaker at `self_ptr`, or no tweaker
/// at all. It takes a pointer because the running `execute` already borrows
/// the tweaker's `RefCell` mutably.
pub(crate) fn check_tweak_drawable_ptr<T: ?Sized>(drawable: &Drawable, self_ptr: *const T) -> bool {
    match &drawable.layer_tweaker {
        None => true,
        Some(tweaker) => std::ptr::addr_eq(tweaker.as_ptr(), self_ptr),
    }
}

/// The tile's render data for `pass`. A free function, so that the result
/// borrows only the tile (the layers mutate themselves while they hold it).
pub(crate) fn render_data_for_pass<'a>(
    tile: &'a RenderTile,
    base_impl: &dyn mln_style::layer::Impl,
    pass: crate::render_pass::RenderPass,
) -> Option<&'a crate::render_layer::LayerRenderData> {
    let render_data = tile.get_layer_render_data(base_impl)?;
    if (render_data.layer_properties.render_passes & pass.underlying_type()) != 0 { Some(render_data) } else { None }
}

#[must_use]
pub(crate) fn cast_matrix(m: &[f64; 16]) -> [f32; 16] {
    std::array::from_fn(|i| m[i] as f32)
}

/// The `RenderLayer` members that manage a tile's drawables
/// (`updateTile`, `removeTile`), for every render layer.
pub(crate) trait TileDrawables: crate::render_layer::RenderLayer {
    /// Removes all drawables for the tile from the layer group.
    fn remove_tile(&mut self, render_pass: crate::render_pass::RenderPass, tile_id: &mln_tile_id::OverscaledTileID) {
        let Some(layer_group) = self.base().layer_group.clone() else {
            return;
        };
        let mut tile_group = layer_group.borrow_mut();
        tile_group.remove_drawables(render_pass, tile_id);
    }

    /// Updates the drawables for a tile. `update` returns true if it updated
    /// the drawable, false if it skipped it because it's for a previous
    /// style. Returns true if drawables were updated.
    fn update_tile(
        &mut self,
        render_pass: crate::render_pass::RenderPass,
        tile_id: &mln_tile_id::OverscaledTileID,
        mut update: impl FnMut(&mut Drawable) -> bool,
    ) -> bool {
        let Some(layer_group) = self.base().layer_group.clone() else {
            return false;
        };

        let mut any_updated = false;
        let mut un_updated_drawables = false;
        {
            let mut tile_group = layer_group.borrow_mut();
            tile_group.visit_drawables_for(render_pass, tile_id, |drawable| {
                if update(drawable) {
                    any_updated = true;
                } else {
                    un_updated_drawables = true;
                }
            });
        }

        // If any are updated, the caller shouldn't add new ones. If none are
        // updated and some were skipped, remove those. This is to handle the
        // case that the style layer changes but the bucket is not re-created.
        if !any_updated && un_updated_drawables {
            self.remove_tile(render_pass, tile_id);
        }

        any_updated
    }
}

impl<T: crate::render_layer::RenderLayer + ?Sized> TileDrawables for T {}
