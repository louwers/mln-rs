// Ports src/mln/tile/tile.hpp, src/mln/tile/tile.cpp,
// include/mln/tile/tile_necessity.hpp and src/mln/tile/tile_operation.hpp.
//
// C++ `Tile` is an abstract class with data members. Tiles are owned by the
// TilePyramid but also receive asynchronous replies (from their worker, the
// file source or the GeoJSON data) on the render thread, which C++ reaches
// through raw pointers. Here a tile's data members are cells in `TileBase`,
// every `Tile` method takes `&self`, and the concrete tiles keep their state
// in an `Rc` that the reply closures reach through a `Weak` (see
// geometry_tile.rs).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_style::expression::GlobalStateMap;
use mln_style::layer_properties::LayerProperties;
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::{Duration, Timestamp};
use mln_util::immutable::Immutable;
use mln_util::logging::{Event, Log};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::tile::tile_observer::{NullTileObserver, TileObserver};
use crate::tile_mask::TileMask;
use crate::tile_render_data::TileRenderData;

/// `TileNecessity`: tiles can have two states: optional or required.
/// - optional means that only low-cost actions should be taken to obtain
///   the data (e.g. load from cache, but accept stale data)
/// - required means that every effort should be taken to obtain the data
///   (e.g. load from internet and keep the data fresh if it expires)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileNecessity {
    Optional,
    Required,
}

/// `TileOperation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TileOperation {
    /// A read request from the cache
    RequestedFromCache,
    /// A read request from the online source
    RequestedFromNetwork,
    /// Tile data from the network has been retrieved
    LoadFromNetwork,
    /// Tile data from the cache has been retrieved
    LoadFromCache,
    /// Background processing of tile data has been initiated
    StartParse,
    /// Background processing of tile data has been completed
    EndParse,
    /// An error occurred while loading the tile
    Error,
    /// Loading of a tile was cancelled
    Cancelled,
    /// No operation has taken place
    NullOp,
}

/// `TileUpdateParameters`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileUpdateParameters {
    pub minimum_update_interval: Duration,
    pub is_volatile: bool,
}

/// `Tile::Kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Geometry,
    Raster,
    RasterDEM,
}

/// The data members of `Tile`.
pub struct TileBase {
    pub kind: Kind,
    id: Cell<OverscaledTileID>,
    pub source_id: String,
    pub modified: Cell<Option<Timestamp>>,
    pub expires: Cell<Option<Timestamp>>,
    /// Indicates whether this tile is used for the currently visible layers
    /// on the map. Re-initialized at every source update.
    pub used_by_rendered_layers: Cell<bool>,

    pub tried_optional: Cell<bool>,
    pub renderable: Cell<bool>,
    pub pending: Cell<bool>,
    pub loaded: Cell<bool>,

    /// Port-only: the necessity that the tile's pyramid last retained the
    /// tile with. A tile retained as optional only looks in the cache, so
    /// one that isn't renderable then loads nothing.
    pub retained_necessity: Cell<TileNecessity>,

    /// The [`SourceFeatureState`](crate::source_state::SourceFeatureState)
    /// version this tile's buckets hold; 0 for buckets that have none.
    pub feature_state_version: Cell<u64>,

    observer: RefCell<Rc<dyn TileObserver>>,
}

impl TileBase {
    /// `Tile(Kind, OverscaledTileID, std::string, TileObserver*)`.
    #[must_use]
    pub fn new(kind: Kind, id: OverscaledTileID, source_id: String, observer: Option<Rc<dyn TileObserver>>) -> Self {
        TileBase {
            kind,
            id: Cell::new(id),
            source_id,
            modified: Cell::new(None),
            expires: Cell::new(None),
            used_by_rendered_layers: Cell::new(false),
            tried_optional: Cell::new(false),
            renderable: Cell::new(false),
            pending: Cell::new(false),
            loaded: Cell::new(false),
            retained_necessity: Cell::new(TileNecessity::Optional),
            feature_state_version: Cell::new(0),
            observer: RefCell::new(observer.unwrap_or_else(|| Rc::new(NullTileObserver))),
        }
    }

    /// The `id` member.
    pub fn id(&self) -> OverscaledTileID {
        self.id.get()
    }

    pub fn set_id(&self, id: OverscaledTileID) {
        self.id.set(id);
    }

    pub fn observer(&self) -> Rc<dyn TileObserver> {
        self.observer.borrow().clone()
    }

    /// `Tile::setObserver(TileObserver*)`.
    pub fn set_observer(&self, observer: Rc<dyn TileObserver>) {
        *self.observer.borrow_mut() = observer;
    }

    /// `setTriedCache()`.
    pub fn set_tried_cache(&self) {
        self.tried_optional.set(true);
        self.observer().on_tile_changed(self);
    }

    /// Returns true when the tile source has received a first response,
    /// regardless of whether a load error occurred or actual data was
    /// loaded.
    pub fn has_tried_cache(&self) -> bool {
        self.tried_optional.get()
    }

    /// Tile data considered "Renderable" can be used for rendering. Data in
    /// partial state is still waiting for network resources but can also be
    /// rendered, although layers will be missing.
    pub fn is_renderable(&self) -> bool {
        self.renderable.get()
    }

    /// A tile is "Loaded" when we have received a response from a
    /// `FileSource`, and have attempted to parse the tile (if applicable).
    /// Tile implementations should set this to true when a load error
    /// occurred, or after the tile was parsed successfully.
    pub fn is_loaded(&self) -> bool {
        self.loaded.get()
    }

    /// "Completion" of a tile means that we have attempted to load it, and
    /// parsed it completely, i.e. no parsing or placement operations are
    /// pending for that tile. Completeness doesn't mean that the tile can be
    /// rendered, but merely that we have exhausted all options to get this
    /// tile to a renderable state. Some tiles may not be renderable, but
    /// complete, e.g. when a raster tile couldn't be loaded, or parsing
    /// failed.
    pub fn is_complete(&self) -> bool {
        self.loaded.get() && !self.pending.get()
    }

    /// `dumpDebugLogs()`.
    pub fn dump_debug_logs(&self) {
        let kind_string = match self.kind {
            Kind::Geometry => "Geometry",
            Kind::Raster => "Raster",
            Kind::RasterDEM => "RasterDEM",
        };
        Log::info(Event::General, format!("Tile::Kind: {kind_string}"));
        Log::info(Event::General, format!("Tile::id: {}", self.id()));
        Log::info(Event::General, format!("Tile::renderable: {}", if self.is_renderable() { "yes" } else { "no" }));
        Log::info(Event::General, format!("Tile::complete: {}", if self.is_complete() { "yes" } else { "no" }));
        Log::info(Event::General, format!("Tile::loaded: {}", if self.is_loaded() { "yes" } else { "no" }));
    }

    /// `onTileAction(TileOperation)` (`TileLoaderObserver`).
    pub fn on_tile_action(&self, op: TileOperation) {
        self.observer().on_tile_action(&self.id(), &self.source_id, op);
    }
}

/// The virtual interface of `Tile`.
pub trait Tile {
    fn base(&self) -> &TileBase;

    fn as_any(&self) -> &dyn Any;

    fn create_render_data(&self) -> Box<dyn TileRenderData>;

    fn set_observer(&self, observer: Rc<dyn TileObserver>) {
        self.base().set_observer(observer);
    }

    fn set_necessity(&self, _necessity: TileNecessity) {}

    fn set_update_parameters(&self, _parameters: &TileUpdateParameters) {}

    /// `retryFailedLoad()`: requests the tile again if its last load failed.
    fn retry_failed_load(&self) {}

    /// Mark this tile as no longer needed and cancel any pending work.
    fn cancel(&self);

    /// Notifies this tile of the updated layer properties.
    ///
    /// Tile implementation should update the contained layer render data
    /// with the given properties.
    ///
    /// Returns `true` if the corresponding render layer data is present in
    /// this tile (and i.e. it was successfully updated); returns `false`
    /// otherwise.
    fn layer_properties_updated(&self, layer_properties: &Immutable<dyn LayerProperties>) -> bool;

    fn set_show_collision_boxes(&self, _show: bool) {}

    /// Takes effect with the next `set_layers`, so a relayout costs one
    /// parse.
    fn set_subdivision_granularity(&self, _value: &SubdivisionGranularitySetting) {}

    /// Sets the pixel ratio the tile lays its layers out for, from the next
    /// `set_layers` on. Returns whether the tile needs laying out again for
    /// it.
    fn set_pixel_ratio(&self, _pixel_ratio: f32) -> bool {
        false
    }

    fn set_layers(&self, _layers: &[Immutable<dyn LayerProperties>], _global_state: Option<Arc<GlobalStateMap>>) {}

    fn set_mask(&self, _mask: TileMask) {}

    // layers: feature state
    /// `setFeatureState(const LayerFeatureStates&)`.
    fn set_feature_state(&self, _states: &crate::bucket::LayerFeatureStates) {}

    /// "holdForFade" is used to keep tiles in the render tree after they're
    /// no longer ideal tiles in order to allow symbols to fade out
    fn hold_for_fade(&self) -> bool {
        false
    }

    /// Set whenever this tile is used as an ideal tile
    fn mark_rendered_ideal(&self) {}

    /// Set when the tile is removed from the ideal render set but may still
    /// be held for fading
    fn mark_rendered_previously(&self) {}

    /// Placement operation performed while this tile is fading. We hold onto
    /// a tile for two placements: fading starts with the first placement and
    /// will have time to finish by the second placement.
    fn performed_fade_placement(&self) {}

    // query: the feature queries (tile.cpp: no-ops for tiles without
    // features).
    /// `queryRenderedFeatures(result, queryGeometry, transformState, layers,
    /// options, globalState, projMatrix, featureState)`.
    #[allow(clippy::too_many_arguments)]
    fn query_rendered_features(
        &self,
        _result: &mut crate::geometry::feature_index::QueryResults,
        _query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        _transform_state: &mln_map_types::TransformState,
        _layers: &crate::geometry::feature_index::QueryLayers<'_>,
        _options: &crate::query::RenderedQueryOptions,
        _global_state: Option<&GlobalStateMap>,
        _proj_matrix: &mln_util::mat4::Mat4,
        _feature_state: &crate::source_state::SourceFeatureState,
    ) -> Result<(), mln_util::error::Error> {
        Ok(())
    }

    /// `querySourceFeatures(result, options, globalState)`.
    fn query_source_features(
        &self,
        _result: &mut Vec<mln_util::feature::Feature>,
        _options: &crate::query::SourceQueryOptions,
        _global_state: Option<&GlobalStateMap>,
    ) -> Result<(), mln_util::error::Error> {
        Ok(())
    }

    /// `getQueryPadding(layers)`.
    fn get_query_padding(&self, _layers: &crate::geometry::feature_index::QueryLayers<'_>) -> f32 {
        0.0
    }

    // Non-virtual members of the C++ class.

    fn id(&self) -> OverscaledTileID {
        self.base().id()
    }

    fn set_tried_cache(&self) {
        self.base().set_tried_cache();
    }

    fn has_tried_cache(&self) -> bool {
        self.base().has_tried_cache()
    }

    fn is_renderable(&self) -> bool {
        self.base().is_renderable()
    }

    fn is_loaded(&self) -> bool {
        self.base().is_loaded()
    }

    fn is_complete(&self) -> bool {
        self.base().is_complete()
    }

    fn dump_debug_logs(&self) {
        self.base().dump_debug_logs();
    }
}
