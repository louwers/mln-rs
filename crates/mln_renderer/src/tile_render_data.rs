use std::rc::Rc;

use mln_style::layer;
use mln_style_types::image::ImagePosition;

use crate::bucket::BucketPtr;
use crate::gfx::context::Context;
use crate::gfx::dynamic_texture::PageTexture;
use crate::gfx::texture2d::Texture2DPtr;
use crate::render_layer::LayerRenderData;
use crate::render_source::SourcePrepareParameters;

/// The glyph and icon (+ pattern) atlas textures of a tile's layout: the
/// pages its glyphs and images are on, whose GPU textures the atlas makes in
/// the frame's upload.
#[derive(Default)]
pub(crate) struct TileAtlasTextures {
    glyph: Option<PageTexture>,
    icon: Option<PageTexture>,
}

impl TileAtlasTextures {
    pub(crate) fn new(glyph: PageTexture, icon: PageTexture) -> Self {
        TileAtlasTextures { glyph: Some(glyph), icon: Some(icon) }
    }

    /// The glyph page's texture, once uploaded.
    pub(crate) fn glyph(&self) -> Option<Texture2DPtr> {
        self.glyph.as_ref().and_then(|texture| texture.get().cloned())
    }

    /// The icon and pattern page's texture, once uploaded.
    pub(crate) fn icon(&self) -> Option<Texture2DPtr> {
        self.icon.as_ref().and_then(|texture| texture.get().cloned())
    }
}

impl std::fmt::Debug for TileAtlasTextures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileAtlasTextures")
            .field("glyph", &self.glyph().is_some())
            .field("icon", &self.icon().is_some())
            .finish()
    }
}

/// A tile's render data. The defaults of `get_pattern`,
/// `get_layer_render_data` and `get_bucket` assert in debug builds: the
/// concrete tile types implement the ones they use.
pub(crate) trait TileRenderData {
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

    fn upload(&self, _context: &Context) {}

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
pub(crate) struct SharedBucketTileRenderData {
    bucket: Option<BucketPtr>,
}

impl SharedBucketTileRenderData {
    #[must_use]
    pub(crate) fn new(bucket: Option<BucketPtr>) -> Self {
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

    fn upload(&self, context: &Context) {
        if let Some(bucket) = &self.bucket {
            bucket.upload(context);
        }
    }
}
