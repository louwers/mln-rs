//! A tile: plain data the tile store owns. It shows its last contents while
//! newer data or work is pending.

use std::sync::Arc;
use std::time::Duration;

use crate::layer::tile_layout::Dependencies;
use crate::log_target;
use crate::map::MapMode;
use crate::storage::Data;
use crate::tile::{OverscaledTileID, TileData};
use crate::tile_store::contents::Contents;
use crate::tile_store::loading::Load;
use crate::tile_store::pipeline::{Layout, Pipeline};

/// An optional tile only takes low-cost actions to get its data, such as
/// loading stale data from the cache; a required one loads it from the
/// network and keeps it fresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TileNecessity {
    Optional,
    Required,
}

/// When a tile's network request starts among the requests that wait for a
/// connection, and its jobs among the other tiles'; lower starts sooner.
/// Every tile's comes after the requests for anything else (styles,
/// sources, sprites, glyphs), which leave theirs at zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TileLoadOrder(u32);

impl TileLoadOrder {
    const RANKS: u32 = 1 << 24;

    /// A tile that no cover of the current view holds.
    pub(crate) const UNWANTED: TileLoadOrder = TileLoadOrder(u32::MAX);

    /// The `rank`th tile that the coarse cover of the current view retains:
    /// there are few of them, and each gives a large part of the view
    /// something to show.
    #[must_use]
    pub(crate) fn coarse(rank: usize) -> Self {
        Self::band(1, rank)
    }

    /// The `rank`th tile that the ideal cover of the current view retains.
    #[must_use]
    pub(crate) fn ideal(rank: usize) -> Self {
        Self::band(2, rank)
    }

    fn band(band: u32, rank: usize) -> Self {
        TileLoadOrder(band * Self::RANKS + (rank as u32).min(Self::RANKS - 1))
    }

    /// The value for a resource's `RequestOrder`.
    #[must_use]
    pub(crate) fn value(self) -> u32 {
        self.0
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TileUpdateParameters {
    pub(crate) minimum_update_interval: Duration,
    pub(crate) is_volatile: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TileKind {
    Geometry,
    Raster,
    Dem(mln_style::tileset::RasterEncoding),
}

/// What a load delivers. `data` is `None` for a tile without any (204, no
/// tile in an archive); `missing`, the tileset has no tile here, so a tile
/// of another zoom level draws in its place.
pub(crate) enum TileInput {
    Geometry { data: Option<TileData>, missing: bool },
    Image { data: Option<Data>, missing: bool },
}

pub(crate) struct Shown {
    pub(crate) contents: Contents,
    /// The feature state version the contents' buckets hold; 0 for none.
    pub(crate) feature_state_version: u64,
    /// The layout's answer resolved glyphs or images that changed since: no
    /// first try starts with it.
    pub(crate) answer_outdated: bool,
    /// The contents are those of the tile in another world copy, without
    /// the layers that are placed, which the tile has yet to lay out.
    pub(crate) world_copy: bool,
}

impl Shown {
    pub(crate) fn new(contents: Contents, feature_state_version: u64) -> Self {
        Shown { contents, feature_state_version, answer_outdated: false, world_copy: false }
    }
}

impl crate::source::source_state::FeatureStateTarget for Shown {
    fn feature_state_version(&mut self) -> &mut u64 {
        &mut self.feature_state_version
    }

    fn set_feature_state(
        &mut self,
        states: &crate::layer::bucket::LayerFeatureStates,
        changed: &mut crate::layer::layer_drawing::ChangedBuckets,
    ) {
        if let Some(contents) = self.contents.geometry_mut() {
            contents.set_feature_state(states, changed);
        }
    }
}

pub(crate) struct Tile {
    pub(crate) id: OverscaledTileID,
    pub(crate) load: Load,
    pub(crate) pipeline: Pipeline,
    pub(crate) shown: Option<Shown>,
    /// The tile has shown contents, failed, or heard its data is still fresh
    /// since it last started loading: it has done what it can for now.
    pub(super) settled: bool,
}

impl Tile {
    pub(crate) fn new(id: OverscaledTileID, load: Load, pipeline: Pipeline) -> Self {
        Tile { id, load, pipeline, shown: None, settled: false }
    }

    pub(crate) fn contents(&self) -> Option<&Contents> {
        self.shown.as_ref().map(|shown| &shown.contents)
    }

    /// The contents, whose buckets the frame changes: their masks, feature
    /// state, placement and upload.
    pub(crate) fn contents_mut(&mut self) -> Option<&mut Contents> {
        self.shown.as_mut().map(|shown| &mut shown.contents)
    }

    /// Whether the tile can draw, if only some of its layers.
    pub(crate) fn is_renderable(&self) -> bool {
        self.contents().is_some_and(Contents::is_renderable)
    }

    pub(crate) fn is_settled(&self) -> bool {
        self.settled
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pipeline.is_pending(self.settled)
    }

    /// Whether the tile has done all it can to become renderable, which it
    /// may not be.
    pub(crate) fn is_complete(&self) -> bool {
        self.settled && !self.is_pending()
    }

    pub(crate) fn has_work(&self) -> bool {
        self.pipeline.has_work()
    }

    /// Whether the tile's render entries are held after they're no longer
    /// ideal, so that its symbols fade out.
    pub(crate) fn fades_out(&self) -> bool {
        self.pipeline.layout().is_some_and(|layout| layout.params.mode == MapMode::Continuous)
    }

    /// Calls `f` with the dependencies of the layout shown and of the one
    /// under way.
    pub(crate) fn dependencies(&self, mut f: impl FnMut(&Dependencies)) {
        if let Some(contents) = self.contents().and_then(Contents::geometry) {
            f(&contents.deps);
        }
        if let Some(deps) = self.pipeline.layout().and_then(Layout::dependencies) {
            f(deps);
        }
    }

    /// Returns whether the tile draws the layer.
    pub(crate) fn layer_properties_updated(
        &mut self,
        layer_properties: &Arc<dyn crate::layer::layer_type::ErasedEvaluatedLayer>,
    ) -> bool {
        self.shown.as_mut().is_some_and(|shown| shown.contents.layer_properties_updated(layer_properties))
    }

    pub(crate) fn dump_debug_logs(&self, kind: TileKind) {
        let kind = match kind {
            TileKind::Geometry => "Geometry",
            TileKind::Raster => "Raster",
            TileKind::Dem(_) => "RasterDEM",
        };
        let yes = |value: bool| if value { "yes" } else { "no" };
        log::info!(target: log_target::GENERAL, "Tile::Kind: {kind}");
        log::info!(target: log_target::GENERAL, "Tile::id: {}", self.id);
        log::info!(target: log_target::GENERAL, "Tile::renderable: {}", yes(self.is_renderable()));
        log::info!(target: log_target::GENERAL, "Tile::complete: {}", yes(self.is_complete()));
        log::info!(target: log_target::GENERAL, "Tile::loaded: {}", yes(self.settled));
    }
}
