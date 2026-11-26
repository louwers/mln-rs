// A render source is the renderer's side of a style source: what every one
// has is `RenderSourceBase`, what each type does the `RenderSource` trait. A
// source's tiles are in the renderer's tile store, which the source's
// methods are given.

use std::sync::Arc;

use mln_style::Source;
use mln_style::error::Error;

use crate::draw::context::Context;
use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::registry::Registry;
use crate::render::paint_parameters::TransformParameters;
use crate::render::tile::RenderTile;
use crate::render::update_parameters::UpdateParameters;
use crate::source::Supplied;
use crate::source::render_image_source::{ImageSourceRenderData, RenderImageSource};
use crate::source::render_tile_source::RenderTileSource;
use crate::tile_store::store::TileStore;
use crate::tile_store::tile_parameters::TileParameters;
use crate::worker::job::SourceKey;

pub(crate) struct RenderTileParameters<'a> {
    pub(crate) transform: &'a TransformParameters,
    /// Whether a layer of the frame draws the source's tiles in
    /// [`TileOrder::ByY`], which `update_render_tiles` then sorts them in.
    pub(crate) sort_by_y: bool,
}

/// What every render source has, whatever its type.
pub(crate) struct RenderSourceBase {
    pub(crate) style_source: Source,
    pub(crate) enabled: bool,
}

impl RenderSourceBase {
    pub(crate) fn new(style_source: Source) -> Self {
        RenderSourceBase { style_source, enabled: false }
    }
}

/// An order a layer draws its source's tiles in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TileOrder {
    /// By tile id, without the tiles held for fade.
    ById,
    /// By opposite y position, so that the lower of two tiles' overlapping
    /// symbols draw over the higher's.
    ByY,
}

pub(crate) trait RenderSource {
    fn base(&self) -> &RenderSourceBase;

    fn is_loaded(&self, store: &TileStore) -> bool;

    fn update(
        &mut self,
        store: &mut TileStore,
        style_source: Source,
        layers: &[Arc<dyn ErasedEvaluatedLayer>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    );

    /// Makes the source's render tiles of the frame.
    fn update_render_tiles(&mut self, store: &mut TileStore, parameters: &RenderTileParameters<'_>);

    /// Readies the buckets the frame draws, before its layers make their
    /// draws, which read what `needs` says, uploading their indexes through
    /// `context`.
    fn upload(&mut self, store: &mut TileStore, context: &mut Context);

    /// Releases the render data from the last `update_render_tiles` while
    /// the source is disabled.
    fn release_render_data(&mut self) {}

    /// The source's tiles in the tile store, for a source with tiles.
    fn tiles(&self) -> Option<SourceKey> {
        None
    }

    /// The source's render tiles, as of the last `update_render_tiles`.
    fn render_tiles(&self) -> &[RenderTile] {
        &[]
    }

    /// The source's tiles in `order`, as indexes into `render_tiles`, as of
    /// the last `update_render_tiles`; `None` for a source without tiles.
    fn tile_order(&self, _order: TileOrder) -> Option<&[usize]> {
        None
    }

    /// An image source's image, as the layers that draw it read it; `None`
    /// for other sources.
    fn image_render_data(&self) -> Option<ImageSourceRenderData<'_>> {
        None
    }

    /// Queries a feature extension; sources without extensions return a null
    /// value.
    fn query_feature_extensions(
        &self,
        _feature: &crate::tile::feature::Feature,
        _extension: &str,
        _extension_field: &str,
        _args: Option<&feature::PropertyMap>,
    ) -> Result<crate::query::FeatureExtensionValue, Error> {
        Ok(crate::query::FeatureExtensionValue::default())
    }

    /// Has the source's origin take in `style_source`, as `update` does
    /// first, so that its tiles' cover is of that version: for a source
    /// another part of the update reads before the sources update.
    fn update_origin(&mut self, _store: &mut TileStore, _style_source: &Source, _parameters: &UpdateParameters) {}

    fn set_fast_pfor_enabled(&mut self, _enable: bool) {}

    fn dump_debug_logs(&self);

    /// The deepest zoom the source has tiles of; `None` for a source without
    /// tiles.
    fn max_zoom(&self) -> Option<u8>;

    fn is_enabled(&self) -> bool {
        self.base().enabled
    }
}

/// Creates the render source of a style source through its type in
/// `registry`, whose tiles, if it has any, `store` keeps. `None` for a type
/// that the registry lacks, which breaks the contract that a map and its
/// renderer register the same types.
pub(crate) fn create(registry: &Registry, source: &Source, store: &mut TileStore) -> Option<Box<dyn RenderSource>> {
    let name = source.info().name;
    let render_source = registry.source_type(name).map(|source_type| -> Box<dyn RenderSource> {
        match source_type.supply(source) {
            Supplied::Tiles(origin) => Box::new(RenderTileSource::new(source.clone(), origin, store)),
            Supplied::Image(image_of) => Box::new(RenderImageSource::new(source.clone(), image_of)),
        }
    });
    debug_assert!(render_source.is_some(), "the renderer's registry has no source type '{name}'");
    render_source
}
