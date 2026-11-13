use std::rc::Rc;

use mln_gfx::UploadPass;
use mln_style::layer;
use mln_style_types::image::ImagePosition;

use crate::bucket::BucketPtr;
use crate::render_layer::LayerRenderData;
use crate::render_source::SourcePrepareParameters;

/// The glyph and icon (+ pattern) atlas textures of a tile. The upload pass
/// sets them through the shared pointer, hence the `RefCell`.
#[derive(Default)]
pub struct TileAtlasTextures {
    pub glyph: std::cell::RefCell<Option<mln_gfx::texture2d::Texture2DPtr>>,
    pub icon: std::cell::RefCell<Option<mln_gfx::texture2d::Texture2DPtr>>,
}

impl std::fmt::Debug for TileAtlasTextures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileAtlasTextures")
            .field("glyph", &self.glyph.borrow().is_some())
            .field("icon", &self.icon.borrow().is_some())
            .finish()
    }
}

/// A tile's render data. The defaults of `get_pattern`,
/// `get_layer_render_data` and `get_bucket` assert in debug builds: the
/// concrete tile types implement the ones they use.
pub trait TileRenderData {
    fn get_atlas_textures(&self) -> Option<&Rc<TileAtlasTextures>>;

    // To be implemented for concrete tile types.
    fn get_pattern(&self, _pattern: &str) -> Option<ImagePosition> {
        debug_assert!(false);
        None
    }

    fn get_layer_render_data(&self, _layer_impl: &dyn layer::Impl) -> Option<&LayerRenderData> {
        debug_assert!(false);
        None
    }

    fn get_bucket(&self, _layer_impl: &dyn layer::Impl) -> Option<&BucketPtr> {
        debug_assert!(false);
        None
    }

    fn upload(&self, _upload_pass: &dyn UploadPass) {}

    /// symbols: the tile's feature index (C++ `GeometryTile::getFeatureIndex`,
    /// reached through `RenderSource::getRenderedTile` in
    /// `RenderSymbolLayer::prepare`).
    fn get_feature_index(&self) -> Option<std::sync::Arc<crate::geometry::feature_index::FeatureIndex>> {
        None
    }

    fn prepare(&mut self, _parameters: &SourcePrepareParameters<'_>) {}

    /// Port-only: the render thread's copies of the buckets' geometry
    /// (`bucket_geometry.rs`).
    fn get_bucket_geometry(&self) -> Option<&crate::bucket_geometry::BucketGeometry> {
        None
    }
}

/// The render data of a tile whose layers share one bucket (raster tiles).
pub struct SharedBucketTileRenderData {
    bucket: Option<BucketPtr>,
}

impl SharedBucketTileRenderData {
    #[must_use]
    pub fn new(bucket: Option<BucketPtr>) -> Self {
        SharedBucketTileRenderData { bucket }
    }
}

impl TileRenderData for SharedBucketTileRenderData {
    fn get_atlas_textures(&self) -> Option<&Rc<TileAtlasTextures>> {
        None
    }

    fn get_bucket(&self, _layer_impl: &dyn layer::Impl) -> Option<&BucketPtr> {
        self.bucket.as_ref()
    }

    fn upload(&self, upload_pass: &dyn UploadPass) {
        if let Some(bucket) = &self.bucket {
            bucket.upload(upload_pass);
        }
    }
}
