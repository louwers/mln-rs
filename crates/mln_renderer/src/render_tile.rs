// A render tile is a tile as a frame draws it: where it draws, its
// matrices, and what it copies of the tile and its render entry (its id,
// hold-for-fade state and whether it draws a visible layer) when the
// frame's update makes it. It lives in its render source, apart from the
// tile store. The frame's phases look the tile's contents up in the store
// by the tile's slot and id (`FrameTile`); the store changes them only in
// the frame's update, before the layers read them.

use std::sync::Arc;

use glam::Vec2;
use mln_map_types::{MapDebugOptions, ProjectionData, TransformState};
use mln_style_types::TranslateAnchorType;
use mln_style_types::image::ImagePosition;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::mat4::Mat4;

use crate::bucket_geometry::BucketGeometry;
use crate::buckets::debug_bucket::DebugBucket;
use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::buckets::raster_bucket::RasterBucket;
use crate::geometry::feature_index::FeatureIndex;
use crate::layer_type::LayerType;
use crate::paint_parameters::TransformParameters;
use crate::render_layer::LayerBucket;
use crate::render_pass::RenderPass;
use crate::render_source::RenderTileParameters;
use crate::tile::contents::Contents;
use crate::tile::tile::Tile;
use crate::tile_render_data::TileAtlasTextures;

pub(crate) struct RenderTile {
    pub(crate) id: UnwrappedTileID,
    /// The projection contract for this tile; `matrix` is its main matrix.
    pub(crate) projection: ProjectionData,
    pub(crate) matrix: Mat4,
    /// The tile's debug overlay, when the map's debug options show one.
    pub(crate) debug_bucket: Option<Box<DebugBucket>>,

    /// `tile.id`.
    tile_id: OverscaledTileID,
    tile_hold_for_fade: bool,
    /// Where the tile is among its source's, which finds its contents
    /// first; `None` for a tile that isn't among a source's.
    slot: Option<usize>,
    needs_rendering: bool,
}

impl RenderTile {
    /// The render tile of `tile`, at `slot` of its source's tiles, drawn as
    /// `id`, before its first update.
    pub(crate) fn new(id: UnwrappedTileID, tile: &Tile, slot: usize, hold_for_fade: bool) -> Self {
        RenderTile { slot: Some(slot), ..RenderTile::of_tile_id(id, tile.id, hold_for_fade) }
    }

    /// The render tile of the tile `tile_id`, drawn as `id`, before it is
    /// updated, which isn't among a source's tiles.
    pub(crate) fn of_tile_id(id: UnwrappedTileID, tile_id: OverscaledTileID, hold_for_fade: bool) -> Self {
        RenderTile {
            id,
            projection: ProjectionData::default(),
            // `set_projection` sets it.
            matrix: [0.0; 16],
            tile_id,
            tile_hold_for_fade: hold_for_fade,
            slot: None,
            needs_rendering: false,
            debug_bucket: None,
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
    pub(crate) fn overscaled_tile_id(&self) -> &OverscaledTileID {
        &self.tile_id
    }

    /// Where the tile is among its source's, as of its last update; `None` for a
    /// tile that isn't among a source's.
    #[must_use]
    pub(crate) fn slot(&self) -> Option<usize> {
        self.slot
    }

    #[must_use]
    pub(crate) fn hold_for_fade(&self) -> bool {
        self.tile_hold_for_fade
    }

    #[must_use]
    pub(crate) fn needs_rendering(&self) -> bool {
        self.needs_rendering
    }

    /// Updates the render tile for the frame; `tile` is the tile it was
    /// created for.
    pub(crate) fn update(&mut self, tile: &Tile, needs_rendering: bool, parameters: &RenderTileParameters<'_>) {
        self.needs_rendering = needs_rendering;
        let (modified, expires) = tile.load.freshness();

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

/// A render tile of a frame, with what its tile shows, as a layer of the
/// type `T` sees it: with the layer's bucket in the tile's layout.
pub(crate) struct FrameTile<'a, T: LayerType> {
    pub(crate) tile: &'a RenderTile,
    contents: Option<&'a Contents>,
    bucket: Option<LayerBucket<'a, T>>,
}

impl<T: LayerType> Clone for FrameTile<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: LayerType> Copy for FrameTile<'_, T> {}

impl<'a, T: LayerType> FrameTile<'a, T> {
    /// The tile `tile`, which shows `contents`, as the layer `layer_id`
    /// sees it.
    #[must_use]
    pub(crate) fn new(tile: &'a RenderTile, contents: Option<&'a Contents>, layer_id: &str) -> Self {
        let bucket =
            contents.and_then(Contents::geometry).and_then(|geometry| geometry.result.typed_bucket::<T>(layer_id));
        FrameTile { tile, contents, bucket }
    }

    /// The layer's bucket in the tile's layout, if the tile laid the layer
    /// out as a layer of its type.
    pub(crate) fn bucket(self) -> Option<LayerBucket<'a, T>> {
        self.bucket
    }

    /// [`bucket`](Self::bucket), if it was laid out to draw in `pass`.
    pub(crate) fn bucket_for_pass(self, pass: RenderPass) -> Option<LayerBucket<'a, T>> {
        self.bucket.filter(|bucket| bucket.layer.render_passes & pass.underlying_type() != 0)
    }

    /// The tile's id.
    #[must_use]
    pub(crate) fn id(self) -> OverscaledTileID {
        *self.tile.overscaled_tile_id()
    }

    /// A raster tile's image.
    #[must_use]
    pub(crate) fn raster(self) -> Option<&'a RasterBucket> {
        match self.contents? {
            Contents::Raster(bucket) => Some(bucket),
            _ => None,
        }
    }

    /// A raster-dem tile's DEM.
    #[must_use]
    pub(crate) fn dem(self) -> Option<&'a HillshadeBucket> {
        match self.contents? {
            Contents::Dem(bucket) => Some(bucket),
            _ => None,
        }
    }

    /// The feature index of the tile's layout.
    #[must_use]
    pub(crate) fn feature_index(self) -> Option<&'a Arc<FeatureIndex>> {
        self.contents?.geometry().map(|contents| &contents.result.feature_index)
    }

    /// The vertex and index data of the tile's buckets, which the frame's
    /// draws bind (`bucket_geometry.rs`).
    #[must_use]
    pub(crate) fn bucket_geometry(self) -> Option<&'a BucketGeometry> {
        self.contents?.geometry().map(|contents| &contents.bucket_geometry)
    }

    /// Where the pattern `pattern` is in the tile's icon atlas.
    #[must_use]
    pub(crate) fn pattern(self, pattern: &str) -> Option<ImagePosition> {
        self.contents?.pattern(pattern)
    }

    /// The textures of the tile's glyph and icon atlases.
    #[must_use]
    pub(crate) fn atlas_textures(self) -> Option<TileAtlasTextures> {
        self.contents?.geometry().map(|contents| contents.textures)
    }
}
