// C++ `RenderTile` keeps a `Tile&`. A RenderTile lives in its render
// source, apart from the tile store, so it copies what it reads from the
// tile and its render entry (its id, hold-for-fade state and whether it
// draws a visible layer) when it is created and prepared. The frame's
// phases borrow the tile's contents from the store by the tile's id
// (`FrameTile`); the store changes them only between frames.

use std::rc::Rc;

use glam::Vec2;
use mln_map_types::{MapDebugOptions, ProjectionData, TransformState};
use mln_style::layer_type::ErasedLayerStyle;
use mln_style_types::TranslateAnchorType;
use mln_style_types::image::ImagePosition;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::mat4::Mat4;

use crate::bucket::Bucket;
use crate::bucket_geometry::BucketGeometry;
use crate::buckets::debug_bucket::DebugBucket; // layers: debug
use crate::paint_parameters::TransformParameters;
use crate::render_layer::LayerRenderData;
use crate::render_source::SourcePrepareParameters;
use crate::tile::contents::Contents;
use crate::tile::tile::Tile;
use crate::tile_render_data::TileAtlasTextures;

pub(crate) struct RenderTile {
    pub(crate) id: UnwrappedTileID,
    /// The projection contract for this tile; `matrix` is its main matrix.
    pub(crate) projection: ProjectionData,
    pub(crate) matrix: Mat4,
    /// layers: debug
    pub(crate) debug_bucket: Option<Box<DebugBucket>>,

    /// `tile.id`.
    tile_id: OverscaledTileID,
    tile_hold_for_fade: bool,
    /// Where the tile is among its source's, which finds its contents.
    slot: usize,
    needs_rendering: bool,
}

impl RenderTile {
    /// The render tile of `tile`, at `slot` of its source's tiles, drawn as
    /// `id`, before it is prepared.
    pub(crate) fn new(id: UnwrappedTileID, tile: &Tile, slot: usize, hold_for_fade: bool) -> Self {
        RenderTile { slot, ..RenderTile::of_tile_id(id, tile.id, hold_for_fade) }
    }

    /// The render tile of the tile `tile_id`, drawn as `id`, before it is
    /// prepared.
    pub(crate) fn of_tile_id(id: UnwrappedTileID, tile_id: OverscaledTileID, hold_for_fade: bool) -> Self {
        RenderTile {
            id,
            projection: ProjectionData::default(),
            // C++ default-constructs `mat4` (uninitialized); prepare sets it.
            matrix: [0.0; 16],
            tile_id,
            tile_hold_for_fade: hold_for_fade,
            slot: usize::MAX,
            needs_rendering: false,
            debug_bucket: None, // layers: debug
        }
    }

    /// The layer's translation in tile units, rotated for its anchor.
    pub(crate) fn tile_unit_translation(
        id: &UnwrappedTileID,
        translation: [f32; 2],
        anchor: TranslateAnchorType,
        state: &TransformState,
    ) -> [f32; 2] {
        if translation[0] == 0.0 && translation[1] == 0.0 {
            return [0.0, 0.0];
        }
        let angle = if anchor == TranslateAnchorType::Viewport { (-state.get_bearing()) as f32 } else { 0.0f32 };
        let translate = Vec2::from_angle(angle).rotate(Vec2::new(translation[0], translation[1]));
        let zoom = state.get_zoom() as f32;
        [id.pixels_to_tile_units(translate.x, zoom), id.pixels_to_tile_units(translate.y, zoom)]
    }

    #[must_use]
    pub(crate) fn get_overscaled_tile_id(&self) -> &OverscaledTileID {
        &self.tile_id
    }

    /// Where the tile is among its source's, as of `prepare`.
    #[must_use]
    pub(crate) fn slot(&self) -> usize {
        self.slot
    }

    #[must_use]
    pub(crate) fn hold_for_fade(&self) -> bool {
        self.tile_hold_for_fade
    }

    #[must_use]
    pub(crate) fn get_needs_rendering(&self) -> bool {
        self.needs_rendering
    }

    /// Prepares the render tile for drawing; `tile` is the tile it was
    /// created for.
    pub(crate) fn prepare(&mut self, tile: &Tile, needs_rendering: bool, parameters: &SourcePrepareParameters<'_>) {
        self.needs_rendering = needs_rendering;
        let (modified, expires) = tile.load.freshness();

        // layers: debug
        let needs_debug_bucket = match &self.debug_bucket {
            None => true,
            Some(debug_bucket) => {
                debug_bucket.renderable != tile.is_renderable()
                    || debug_bucket.complete != tile.is_complete()
                    || debug_bucket.modified != modified
                    || debug_bucket.expires != expires
                    || debug_bucket.debug_mode != parameters.debug_options
            }
        };
        if parameters.debug_options != MapDebugOptions::NO_DEBUG && needs_debug_bucket {
            self.debug_bucket = Some(Box::new(DebugBucket::new(
                &tile.id,
                tile.is_renderable(),
                tile.is_complete(),
                modified,
                expires,
                parameters.debug_options,
                &parameters.source_name,
            )));
        } else if parameters.debug_options == MapDebugOptions::NO_DEBUG {
            self.debug_bucket = None;
        }

        self.set_projection(parameters.transform);
    }

    /// Sets the tile's projection and its standard matrix for `transform`.
    pub(crate) fn set_projection(&mut self, transform: &TransformParameters) {
        self.projection = transform.state.get_projection_data_with(&self.id, &transform.proj_matrix);
        self.matrix = self.projection.main_matrix;
    }
}

/// A render tile of a frame, with what its tile shows.
#[derive(Clone, Copy)]
pub(crate) struct FrameTile<'a> {
    pub(crate) tile: &'a RenderTile,
    contents: Option<&'a Contents>,
}

impl<'a> FrameTile<'a> {
    #[must_use]
    pub(crate) fn new(tile: &'a RenderTile, contents: Option<&'a Contents>) -> Self {
        FrameTile { tile, contents }
    }

    /// The bucket of `impl_`'s layer; a raster or raster-dem tile's for any
    /// layer.
    pub(crate) fn get_bucket(self, impl_: &dyn ErasedLayerStyle) -> Option<&'a dyn Bucket> {
        self.contents?.get_bucket(impl_)
    }

    /// symbols: the feature index of the tile's layout result.
    #[must_use]
    pub(crate) fn get_feature_index(self) -> Option<std::sync::Arc<crate::geometry::feature_index::FeatureIndex>> {
        self.contents?.geometry().map(|contents| std::sync::Arc::clone(&contents.result.feature_index))
    }

    /// Port-only: the render thread's copies of the tile's bucket geometry
    /// (`bucket_geometry.rs`).
    #[must_use]
    pub(crate) fn get_bucket_geometry(self) -> Option<&'a BucketGeometry> {
        self.contents?.geometry().map(|contents| &contents.bucket_geometry)
    }

    pub(crate) fn get_layer_render_data(self, impl_: &dyn ErasedLayerStyle) -> Option<LayerRenderData<'a>> {
        self.contents?.geometry()?.result.get_layer_render_data(impl_)
    }

    #[must_use]
    pub(crate) fn get_pattern(self, pattern: &str) -> Option<ImagePosition> {
        self.contents?.get_pattern(pattern)
    }

    #[must_use]
    pub(crate) fn get_atlas_textures(self) -> Option<&'a Rc<TileAtlasTextures>> {
        self.contents?.geometry().map(|contents| &contents.textures)
    }
}

impl std::ops::Deref for FrameTile<'_> {
    type Target = RenderTile;

    fn deref(&self) -> &RenderTile {
        self.tile
    }
}
