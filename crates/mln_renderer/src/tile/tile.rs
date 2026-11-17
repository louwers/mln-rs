//! A tile: plain data the tile store owns and changes through `&mut`. Its
//! load, its pipeline and what it shows are separate fields, since a tile
//! shows its last contents while newer data or work is pending.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mln_map_types::MapMode;
use mln_storage::Data;
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::logging::{Event, Log};

use crate::layout::tile_layout::Dependencies;
use crate::tile::contents::Contents;
use crate::tile::loading::Load;
use crate::tile::pipeline::{Layout, Pipeline};

/// Tiles can have two states: optional or required.
/// - optional means that only low-cost actions should be taken to obtain
///   the data (e.g. load from cache, but accept stale data)
/// - required means that every effort should be taken to obtain the data
///   (e.g. load from internet and keep the data fresh if it expires)
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

/// What a source's tiles hold, and which job turns their data into
/// contents.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TileKind {
    /// Vector data, laid out into buckets.
    Geometry,
    /// An image, decoded for raster layers.
    Raster,
    /// A DEM of this encoding, decoded for hillshade and color-relief layers.
    Dem(mln_util::tileset::RasterEncoding),
}

/// What a load delivers: the data of a geometry tile, or the image of a
/// raster or raster-dem tile. `missing`: the tileset has no tile here, so a
/// tile of another zoom level draws in its place.
pub(crate) enum TileInput {
    /// `data` is `None` for a tile without any (204, no tile in an archive).
    Geometry { data: Option<TileData>, missing: bool },
    /// `data` is `None` for a tile without an image, which shows nothing.
    Image { data: Option<Data>, missing: bool },
}

/// What a tile shows, and what is known of it.
pub(crate) struct Shown {
    /// `Rc` while frames draw it too.
    pub(crate) contents: Rc<Contents>,
    /// The [`SourceFeatureState`](crate::source_state::SourceFeatureState)
    /// version the contents' buckets hold; 0 for buckets that have none.
    pub(crate) feature_state_version: u64,
    /// The layout's answer resolved glyphs or images that changed since: no
    /// first try starts with it.
    pub(crate) answer_outdated: bool,
}

impl Shown {
    /// `contents`, as installed: up to date.
    pub(crate) fn new(contents: Contents, feature_state_version: u64) -> Self {
        Shown { contents: Rc::new(contents), feature_state_version, answer_outdated: false }
    }
}

impl crate::source_state::FeatureStateTarget for Shown {
    fn feature_state_version(&mut self) -> &mut u64 {
        &mut self.feature_state_version
    }

    fn set_feature_state(&self, states: &crate::bucket::LayerFeatureStates) {
        if let Some(contents) = self.contents.geometry() {
            contents.set_feature_state(states);
        }
    }
}

/// Where a render entry stands in fading its symbols out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FadeState {
    Loaded,
    NeedsFirstPlacement,
    NeedsSecondPlacement,
    CanRemove,
}

impl FadeState {
    /// The state of an entry rendered by the last update but not this one:
    /// an entry that was rendered as an ideal tile starts to fade out.
    #[must_use]
    pub(crate) fn rendered_previously(self) -> Self {
        match self {
            FadeState::Loaded => FadeState::NeedsFirstPlacement,
            other => other,
        }
    }

    /// The state after a placement while the entry fades: fading starts
    /// with the first placement and has time to finish by the second.
    #[must_use]
    pub(crate) fn stepped(self) -> Self {
        match self {
            FadeState::NeedsFirstPlacement => FadeState::NeedsSecondPlacement,
            FadeState::NeedsSecondPlacement => FadeState::CanRemove,
            other => other,
        }
    }

    /// Whether the entry stays in the render tree, no longer ideal, so that
    /// its symbols fade out.
    #[must_use]
    pub(crate) fn holds(self) -> bool {
        matches!(self, FadeState::NeedsFirstPlacement | FadeState::NeedsSecondPlacement)
    }
}

/// A tile of a source.
pub(crate) struct Tile {
    pub(crate) id: OverscaledTileID,
    /// Where the tile's data comes from, and how much it is needed.
    pub(crate) load: Load,
    /// How the tile's data becomes its contents.
    pub(crate) pipeline: Pipeline,
    /// What the tile draws.
    pub(crate) shown: Option<Shown>,
    /// The tile has shown contents, failed, or heard that its data is
    /// still fresh since it last started loading: it has done what it can
    /// for now. Only its transitions (`TileMut`) write it.
    pub(super) settled: bool,
}

impl Tile {
    pub(crate) fn new(id: OverscaledTileID, load: Load, pipeline: Pipeline) -> Self {
        Tile { id, load, pipeline, shown: None, settled: false }
    }

    /// What the tile draws.
    pub(crate) fn contents(&self) -> Option<&Contents> {
        self.shown.as_ref().map(|shown| &*shown.contents)
    }

    /// Tile data considered "Renderable" can be used for rendering. Data in
    /// partial state is still waiting for network resources but can also be
    /// rendered, although layers will be missing.
    pub(crate) fn is_renderable(&self) -> bool {
        self.contents().is_some_and(Contents::is_renderable)
    }

    /// Whether the tile has shown contents, failed, or heard that its data
    /// is still fresh since it last started loading: whether it has done
    /// what it can for now.
    pub(crate) fn is_settled(&self) -> bool {
        self.settled
    }

    /// Whether the tile has work under way or to start: a job in flight or
    /// waiting, or one to run for the data it has.
    pub(crate) fn is_pending(&self) -> bool {
        self.pipeline.is_pending(self.settled)
    }

    /// "Completion" of a tile means that we have attempted to load it, and
    /// parsed it completely, i.e. no parsing or placement operations are
    /// pending for that tile. Completeness doesn't mean that the tile can be
    /// rendered, but merely that we have exhausted all options to get this
    /// tile to a renderable state.
    pub(crate) fn is_complete(&self) -> bool {
        self.settled && !self.is_pending()
    }

    /// Whether the tile has a job it can submit, or dependencies to resolve.
    pub(crate) fn has_work(&self) -> bool {
        self.pipeline.has_work()
    }

    /// Whether the tile's render entries are held after they're no longer
    /// ideal, so that its symbols fade out: a geometry tile of a continuous
    /// map.
    pub(crate) fn fades_out(&self) -> bool {
        self.pipeline.layout().is_some_and(|layout| layout.params().mode == MapMode::Continuous)
    }

    /// Calls `f` with the dependencies of the layout the tile shows and of
    /// the one it lays out.
    pub(crate) fn dependencies(&self, mut f: impl FnMut(&Dependencies)) {
        if let Some(contents) = self.contents().and_then(Contents::geometry) {
            f(&contents.deps);
        }
        if let Some(deps) = self.pipeline.layout().and_then(Layout::dependencies) {
            f(deps);
        }
    }

    /// Notifies the tile of a visible layer's updated properties. Returns
    /// whether the tile draws the layer.
    pub(crate) fn layer_properties_updated(
        &mut self,
        layer_properties: &Arc<dyn crate::layer_type::ErasedEvaluatedLayer>,
    ) -> bool {
        self.shown
            .as_mut()
            .is_some_and(|shown| Contents::layer_properties_updated(&mut shown.contents, layer_properties))
    }

    /// Logs the tile's state, as a tile of `kind`.
    pub(crate) fn dump_debug_logs(&self, kind: TileKind) {
        let kind = match kind {
            TileKind::Geometry => "Geometry",
            TileKind::Raster => "Raster",
            TileKind::Dem(_) => "RasterDEM",
        };
        let yes = |value: bool| if value { "yes" } else { "no" };
        Log::info(Event::General, format!("Tile::Kind: {kind}"));
        Log::info(Event::General, format!("Tile::id: {}", self.id));
        Log::info(Event::General, format!("Tile::renderable: {}", yes(self.is_renderable())));
        Log::info(Event::General, format!("Tile::complete: {}", yes(self.is_complete())));
        Log::info(Event::General, format!("Tile::loaded: {}", yes(self.settled)));
    }
}
