// C++ `RenderTile` keeps a `Tile&`. A Rust RenderTile cannot borrow the tile
// out of the TilePyramid for the frame, so it copies what it reads from the
// tile (its id and hold-for-fade state) when it is created and prepared;
// both are only changed by the TilePyramid between frames.

use std::rc::Rc;

use glam::Vec2;
use mln_gfx::UploadPass;
use mln_map_types::{MapDebugOptions, ProjectionData, TransformState};
use mln_style::layer;
use mln_style_types::TranslateAnchorType;
use mln_style_types::image::ImagePosition;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::mat4::Mat4;

use crate::bucket::BucketPtr;
use crate::buckets::debug_bucket::DebugBucket; // layers: debug
use crate::render_layer::LayerRenderData;
use crate::render_source::SourcePrepareParameters;
use crate::tile::tile::Tile;
use crate::tile_render_data::{TileAtlasTextures, TileRenderData};

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
    // The following members are reset at placement stage.
    render_data: Option<Box<dyn TileRenderData>>,
    needs_rendering: bool,
}

impl RenderTile {
    pub(crate) fn new(id: UnwrappedTileID, tile: &dyn Tile) -> Self {
        RenderTile {
            id,
            projection: ProjectionData::default(),
            // C++ default-constructs `mat4` (uninitialized); prepare sets it.
            matrix: [0.0; 16],
            tile_id: tile.id(),
            tile_hold_for_fade: tile.hold_for_fade(),
            render_data: None,
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

    #[must_use]
    pub(crate) fn hold_for_fade(&self) -> bool {
        self.tile_hold_for_fade
    }

    fn render_data(&self) -> &dyn TileRenderData {
        // C++ asserts renderData.
        self.render_data.as_deref().expect("RenderTile::prepare")
    }

    pub(crate) fn get_bucket(&self, impl_: &dyn layer::Impl) -> Option<&BucketPtr> {
        self.render_data().get_bucket(impl_)
    }

    /// symbols: the feature index of the tile's layout result.
    #[must_use]
    pub(crate) fn get_feature_index(&self) -> Option<std::sync::Arc<crate::geometry::feature_index::FeatureIndex>> {
        self.render_data.as_ref().and_then(|data| data.get_feature_index())
    }

    /// Port-only: the render thread's copies of the tile's bucket geometry
    /// (`bucket_geometry.rs`).
    #[must_use]
    pub(crate) fn get_bucket_geometry(&self) -> Option<&crate::bucket_geometry::BucketGeometry> {
        self.render_data.as_deref().and_then(|data| data.get_bucket_geometry())
    }

    pub(crate) fn get_layer_render_data(&self, impl_: &dyn layer::Impl) -> Option<&LayerRenderData> {
        self.render_data().get_layer_render_data(impl_)
    }

    #[must_use]
    pub(crate) fn get_pattern(&self, pattern: &str) -> Option<ImagePosition> {
        self.render_data().get_pattern(pattern)
    }

    #[must_use]
    pub(crate) fn get_atlas_textures(&self) -> Option<&Rc<TileAtlasTextures>> {
        self.render_data.as_deref().and_then(|data| data.get_atlas_textures())
    }

    #[must_use]
    pub(crate) fn get_needs_rendering(&self) -> bool {
        self.needs_rendering
    }

    pub(crate) fn upload(&self, upload_pass: &dyn UploadPass) {
        self.render_data().upload(upload_pass);
    }

    /// Prepares the render tile for drawing; `tile` is the tile it was
    /// created for.
    pub(crate) fn prepare(&mut self, tile: &dyn Tile, parameters: &SourcePrepareParameters<'_>) {
        let mut render_data = tile.create_render_data();
        render_data.prepare(parameters);
        self.render_data = Some(render_data);

        self.needs_rendering = tile.base().used_by_rendered_layers.get();

        // layers: debug
        let tile_base = tile.base();
        let needs_debug_bucket = match &self.debug_bucket {
            None => true,
            Some(debug_bucket) => {
                debug_bucket.renderable != tile.is_renderable()
                    || debug_bucket.complete != tile.is_complete()
                    || debug_bucket.modified != tile_base.modified.get()
                    || debug_bucket.expires != tile_base.expires.get()
                    || debug_bucket.debug_mode != parameters.debug_options
            }
        };
        if parameters.debug_options != MapDebugOptions::NO_DEBUG && needs_debug_bucket {
            self.debug_bucket = Some(Box::new(DebugBucket::new(
                &tile.id(),
                tile.is_renderable(),
                tile.is_complete(),
                tile_base.modified.get(),
                tile_base.expires.get(),
                parameters.debug_options,
                &parameters.source_name,
            )));
        } else if parameters.debug_options == MapDebugOptions::NO_DEBUG {
            self.debug_bucket = None;
        }

        // Calculate the standard tile matrix.
        let transform = parameters.transform;
        self.projection = transform.state.get_projection_data_with(&self.id, &transform.proj_matrix);
        self.matrix = self.projection.main_matrix;
    }
}
