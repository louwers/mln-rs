// C++ `RenderSource` is an abstract class with data members; the data is
// `RenderSourceBase`. A source's tiles are in the renderer's tile store,
// which the source's methods are given.

use std::rc::Rc;
use std::sync::Arc;

use mln_map_types::MapDebugOptions;
use mln_style::Source;
use mln_util::constants;
use mln_util::error::Error;

use crate::layer_type::ErasedEvaluatedLayer;
use crate::paint_parameters::TransformParameters;
use crate::registry::Registry;
use crate::render_tile::RenderTile;
use crate::render_tree::RenderItem;
use crate::tile::job::SourceKey;
use crate::tile::store::TileStore;
use crate::tile_parameters::TileParameters;

pub(crate) struct SourcePrepareParameters<'a> {
    pub(crate) transform: &'a TransformParameters,
    pub(crate) debug_options: MapDebugOptions,
    pub(crate) source_name: String,
}

/// A shared selection of the render tiles of an `Rc<Vec<RenderTile>>`,
/// which it keeps alive.
#[derive(Clone)]
pub(crate) struct RenderTiles {
    owner: Rc<Vec<RenderTile>>,
    indices: Rc<Vec<usize>>,
}

impl RenderTiles {
    #[must_use]
    pub(crate) fn new(owner: Rc<Vec<RenderTile>>, indices: Vec<usize>) -> Self {
        RenderTiles { owner, indices: Rc::new(indices) }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &RenderTile> + '_ {
        self.indices.iter().map(|&i| &self.owner[i])
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// The tiles the selection points into.
    #[must_use]
    pub(crate) fn owner(&self) -> &Rc<Vec<RenderTile>> {
        &self.owner
    }
}

/// The data members of `RenderSource`.
pub(crate) struct RenderSourceBase {
    pub(crate) style_source: Source,
    pub(crate) enabled: bool,
}

impl RenderSourceBase {
    pub(crate) fn new(style_source: Source) -> Self {
        RenderSourceBase { style_source, enabled: false }
    }
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

    /// Note: calling of this method might reset the contained render data.
    fn create_render_item(&mut self) -> Box<dyn RenderItem>;

    /// Creates the render data to be passed to the render item.
    fn prepare(&mut self, store: &mut TileStore, parameters: &SourcePrepareParameters<'_>);

    /// Releases the render data from the last `prepare` while the source is
    /// disabled.
    fn release_render_data(&mut self) {}

    /// The source's tiles in the tile store, for a source with tiles.
    fn tiles(&self) -> Option<SourceKey> {
        None
    }

    /// If supported, returns a shared list of `RenderTiles`, sorted by tile id
    /// and excluding tiles hold for fade; returns `None` otherwise.
    fn get_render_tiles(&self) -> Option<RenderTiles> {
        None
    }

    /// If supported, returns a shared list of `RenderTiles`, sorted in
    /// opposite y position, so tiles with overlapping symbols are drawn on
    /// top of each other, with lower symbols being drawn on top of higher
    /// symbols; returns `None` otherwise.
    fn get_render_tiles_sorted_by_y_position(&self) -> Option<RenderTiles> {
        None
    }

    /// conformance: `getImageRenderData()` (image sources).
    fn get_image_render_data(&self) -> Option<Rc<crate::sources::render_image_source::ImageSourceRenderData>> {
        None
    }

    fn get_raw_render_tiles(&self) -> Rc<Vec<RenderTile>> {
        Rc::new(Vec::new())
    }

    /// Queries a feature extension; sources without extensions return a null
    /// value.
    fn query_feature_extensions(
        &self,
        _feature: &mln_util::feature::Feature,
        _extension: &str,
        _extension_field: &str,
        _args: Option<&feature::PropertyMap>,
    ) -> Result<crate::query::FeatureExtensionValue, Error> {
        Ok(crate::query::FeatureExtensionValue::default())
    }

    fn set_fast_pfor_enabled(&mut self, _enable: bool) {}

    fn dump_debug_logs(&self);

    fn get_max_zoom(&self) -> u8 {
        debug_assert!(false);
        constants::TERRAIN_RGB_MAXZOOM
    }

    // Non-virtual members of the C++ class.

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
    let render_source = registry.source_type(name).map(|source_type| source_type.create_render_source(source, store));
    debug_assert!(render_source.is_some(), "the renderer's registry has no source type '{name}'");
    render_source
}
