//! A tile: plain data the tile store owns and changes through `&mut`. Its
//! data, its work and its contents are separate fields, since a tile shows
//! its last contents while newer data or work is pending.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mln_map_types::MapMode;
use mln_storage::{Data, Request};
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::Timestamp;
use mln_util::logging::{Event, Log};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::image_store::ImageWait;
use crate::layout::tile_layout::{Dependencies, LayoutParams};
use crate::text::glyph_store::GlyphWait;
use crate::tile::contents::{Answer, Contents};
use crate::tile::dem::DEMTileNeighbors;
use crate::tile::job::{DataKey, JobId, SourceStyle};
use crate::tile::loading::TileLoad;
use crate::tile::tile_workers::Lane;

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

/// What a job reads: the data of one load. The store keeps a geometry
/// tile's until the tile drops, and a raster tile's until its decode
/// replies, so a job whose lane is lost can always run again.
pub(crate) enum TileInput {
    /// `data` is `None` for a tile without any (204, no tile in an archive).
    /// `missing`: the tileset has no tile here, so a tile of another zoom
    /// level draws in its place.
    Geometry { data: Option<TileData>, missing: bool },
    /// The image to decode, until its decode replies.
    Image(Option<Data>),
}

/// The data of one load.
pub(crate) struct Loaded {
    pub(crate) key: DataKey,
    pub(crate) input: TileInput,
}

impl Loaded {
    /// The geometry data, if the load has any.
    pub(crate) fn data(&self) -> Option<&TileData> {
        match &self.input {
            TileInput::Geometry { data, .. } => data.as_ref(),
            TileInput::Image(_) => None,
        }
    }

    fn missing(&self) -> bool {
        matches!(self.input, TileInput::Geometry { missing: true, .. })
    }
}

/// Where a tile's data stands. Data events name the load they answer (a
/// value of the source's counter) and apply only to it, never comparing the
/// layout's generation, so a style edit doesn't drop data that is loading.
/// A load that fails leaves the tile with the data it had.
pub(crate) enum DataState {
    /// Nothing asked for yet, or the last request was cancelled or failed.
    Unrequested,
    /// Load `load` is under way: `request` while a file request answers it.
    /// `current` is the data of the load before, which a reload keeps
    /// showing and laying out, and a `not_modified` response keeps.
    Loading { load: u64, request: Option<Request>, current: Option<Loaded> },
    /// The data of load `load`. A custom source can send it again.
    Loaded { load: u64, data: Loaded },
}

impl DataState {
    /// The data the tile lays out with.
    pub(crate) fn current(&self) -> Option<&Loaded> {
        match self {
            DataState::Loading { current, .. } => current.as_ref(),
            DataState::Loaded { data, .. } => Some(data),
            DataState::Unrequested => None,
        }
    }

    pub(crate) fn current_mut(&mut self) -> Option<&mut Loaded> {
        match self {
            DataState::Loading { current, .. } => current.as_mut(),
            DataState::Loaded { data, .. } => Some(data),
            DataState::Unrequested => None,
        }
    }

    /// Takes the data the tile lays out with, leaving the state otherwise.
    pub(crate) fn take_current(&mut self) -> Option<Loaded> {
        match std::mem::replace(self, DataState::Unrequested) {
            DataState::Loading { load, request, current } => {
                *self = DataState::Loading { load, request, current: None };
                current
            }
            DataState::Loaded { data, .. } => Some(data),
            DataState::Unrequested => None,
        }
    }

    /// The load the data events that apply name.
    pub(crate) fn load(&self) -> Option<u64> {
        match self {
            DataState::Loading { load, .. } | DataState::Loaded { load, .. } => Some(*load),
            DataState::Unrequested => None,
        }
    }

    /// The file request under way, if any.
    pub(crate) fn request(&self) -> Option<&Request> {
        match self {
            DataState::Loading { request, .. } => request.as_ref(),
            DataState::Unrequested | DataState::Loaded { .. } => None,
        }
    }
}

/// A job in flight, of the tile's current generation or (when stale) an
/// earlier one.
pub(crate) struct Running {
    pub(crate) id: JobId,
    pub(crate) lane: Lane,
    /// The job is a resume, whose `answer` resolves `known`.
    pub(crate) known: Option<Dependencies>,
    pub(crate) answer: Answer,
    /// The images that exist, as the job got them.
    pub(crate) images: Arc<VecSet<String>>,
    pub(crate) superseded: Arc<AtomicBool>,
}

/// A wait for the stores to resolve a job's dependencies.
#[derive(Default)]
pub(crate) struct DependencyWait {
    pub(crate) glyphs: GlyphWait,
    pub(crate) images: ImageWait,
    /// The stores' revisions at the last try, which only another revision
    /// can change the outcome of.
    pub(crate) revisions: Option<(u64, u64)>,
}

/// A job that answered `NeedsDeps`: the tile waits for the stores.
pub(crate) struct Waiting {
    pub(crate) lane: Lane,
    pub(crate) needs: Dependencies,
    pub(crate) wait: DependencyWait,
    /// The images the job got, which its resume discovers with again.
    pub(crate) images: Arc<VecSet<String>>,
}

/// The next job, ready but waiting for a lane with room: a resume with its
/// answer, or a job sent again with its data.
pub(crate) struct Next {
    /// The lane of the job before, for a resume.
    pub(crate) lane: Option<Lane>,
    pub(crate) known: Option<Dependencies>,
    pub(crate) answer: Answer,
    /// The images the job before got, which a resume discovers with again;
    /// `None` for a first try, which gets the images as they are.
    pub(crate) images: Option<Arc<VecSet<String>>>,
}

/// Where a tile's work stands.
pub(crate) enum WorkState {
    /// Nothing to do for this generation: the tile shows its contents,
    /// failed, or waits for the data or style it lacks.
    Current,
    /// The generation has no job yet.
    Outdated,
    Running(Running),
    Waiting(Waiting),
    Ready(Next),
}

impl WorkState {
    /// The dependencies the work names, apart from what the tile shows.
    pub(crate) fn dependencies(&self) -> Option<&Dependencies> {
        match self {
            WorkState::Running(Running { known, .. }) | WorkState::Ready(Next { known, .. }) => known.as_ref(),
            WorkState::Waiting(Waiting { needs, .. }) => Some(needs),
            WorkState::Current | WorkState::Outdated => None,
        }
    }

    /// The lane the work holds a place on, or whose worker keeps its state.
    pub(crate) fn lane(&self) -> Option<Lane> {
        match self {
            WorkState::Running(Running { lane, .. })
            | WorkState::Waiting(Waiting { lane, .. })
            | WorkState::Ready(Next { lane: Some(lane), .. }) => Some(*lane),
            WorkState::Current | WorkState::Outdated | WorkState::Ready(_) => None,
        }
    }
}

/// Where a tile stands in fading its symbols out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FadeState {
    Loaded,
    NeedsFirstPlacement,
    NeedsSecondPlacement,
    CanRemove,
}

/// A tile of a source.
pub(crate) struct Tile {
    pub(crate) id: OverscaledTileID,
    pub(crate) kind: TileKind,
    /// The generation of the tile's inputs, from its source's counter.
    pub(crate) generation: u64,
    pub(crate) data: DataState,
    pub(crate) work: WorkState,
    /// What the tile draws; `Rc` while frames draw it too.
    pub(crate) contents: Option<Rc<Contents>>,
    /// The tile has had a response, a layout or an error since it last
    /// started loading: it has exhausted what it can do for now.
    pub(crate) loaded: bool,
    /// The tile has had its first response.
    pub(crate) tried_cache: bool,
    pub(crate) modified: Option<Timestamp>,
    pub(crate) expires: Option<Timestamp>,
    pub(crate) load: TileLoad,

    // Layout inputs, for a geometry tile.
    /// The source's style, from the tile's last `set_style`.
    pub(crate) style: Option<Arc<SourceStyle>>,
    /// The parameters of the next job; `first_load` is set as it starts.
    pub(crate) params: LayoutParams,
    /// The pixel ratio and granularity the next `set_style` lays out for.
    pub(crate) pixel_ratio: f32,
    pub(crate) granularity: SubdivisionGranularitySetting,
    /// The lane that last got the data lost it: the next job carries it.
    pub(crate) full: bool,
    /// The shown layout's answer resolved glyphs or images that changed
    /// since: the next first try starts without it.
    pub(crate) answer_outdated: bool,

    // The source's bookkeeping.
    /// Whether the tile is in the source's cache rather than its cover.
    pub(crate) cached: bool,
    /// The necessity the cover last retained the tile with. A tile retained
    /// as optional only looks in the cache, so one that isn't renderable
    /// then loads nothing.
    pub(crate) retained_necessity: TileNecessity,
    /// Where the cover last ranked the tile, which orders its requests and
    /// jobs among the other tiles'.
    pub(crate) load_order: TileLoadOrder,
    /// Whether the tile is used for the visible layers; set at every update.
    pub(crate) used_by_rendered_layers: bool,
    pub(crate) fade: FadeState,
    /// The [`SourceFeatureState`](crate::source_state::SourceFeatureState)
    /// version the contents' buckets hold; 0 for buckets that have none.
    pub(crate) feature_state_version: u64,
    /// raster-dem: the neighbours whose borders the DEM has (a bit each).
    pub(crate) dem_neighbors: DEMTileNeighbors,
}

impl Tile {
    pub(crate) fn new(
        id: OverscaledTileID,
        kind: TileKind,
        generation: u64,
        load: TileLoad,
        params: LayoutParams,
    ) -> Self {
        Tile {
            id,
            kind,
            generation,
            data: DataState::Unrequested,
            work: WorkState::Outdated,
            contents: None,
            loaded: false,
            tried_cache: false,
            modified: None,
            expires: None,
            load,
            style: None,
            pixel_ratio: params.pixel_ratio,
            granularity: params.subdivision_granularity,
            params,
            full: false,
            answer_outdated: false,
            cached: false,
            retained_necessity: TileNecessity::Optional,
            load_order: TileLoadOrder::UNWANTED,
            used_by_rendered_layers: false,
            fade: FadeState::Loaded,
            feature_state_version: 0,
            dem_neighbors: DEMTileNeighbors::of(&id),
        }
    }

    /// Takes a new generation: a job in flight becomes stale, and any other
    /// work outdated.
    pub(crate) fn bump(&mut self, generation: u64) {
        self.generation = generation;
        match &self.work {
            WorkState::Running(running) => running.superseded.store(true, Ordering::Relaxed),
            _ => self.work = WorkState::Outdated,
        }
    }

    /// Tile data considered "Renderable" can be used for rendering. Data in
    /// partial state is still waiting for network resources but can also be
    /// rendered, although layers will be missing.
    pub(crate) fn is_renderable(&self) -> bool {
        self.contents.as_ref().is_some_and(|contents| contents.is_renderable())
    }

    /// Whether the tile has work under way or to start: a job in flight or
    /// waiting, or one to run for the data it has. A geometry tile with its
    /// style counts its layout as started from its first load on, as C++'s
    /// does from `setLayers`.
    pub(crate) fn is_pending(&self) -> bool {
        match &self.work {
            WorkState::Current => false,
            WorkState::Outdated => match self.kind {
                TileKind::Geometry => self.style.is_some() && (self.data.current().is_some() || !self.loaded),
                TileKind::Raster | TileKind::Dem(_) => {
                    matches!(self.data.current(), Some(Loaded { input: TileInput::Image(Some(_)), .. }))
                }
            },
            WorkState::Running(_) | WorkState::Waiting(_) | WorkState::Ready(_) => true,
        }
    }

    /// "Completion" of a tile means that we have attempted to load it, and
    /// parsed it completely, i.e. no parsing or placement operations are
    /// pending for that tile. Completeness doesn't mean that the tile can be
    /// rendered, but merely that we have exhausted all options to get this
    /// tile to a renderable state.
    pub(crate) fn is_complete(&self) -> bool {
        self.loaded && !self.is_pending()
    }

    /// Whether the data the tile lays out with is missing from the tileset.
    pub(crate) fn data_missing(&self) -> bool {
        self.data.current().is_some_and(Loaded::missing)
    }

    /// "holdForFade" keeps tiles in the render tree after they're no longer
    /// ideal tiles, so that their symbols fade out.
    pub(crate) fn hold_for_fade(&self) -> bool {
        self.kind == TileKind::Geometry
            && self.params.mode == MapMode::Continuous
            && matches!(self.fade, FadeState::NeedsFirstPlacement | FadeState::NeedsSecondPlacement)
    }

    /// Set whenever this tile is used as an ideal tile.
    pub(crate) fn mark_rendered_ideal(&mut self) {
        self.fade = FadeState::Loaded;
    }

    /// Set when the tile is removed from the ideal render set but may still
    /// be held for fading.
    pub(crate) fn mark_rendered_previously(&mut self) {
        if self.fade == FadeState::Loaded {
            self.fade = FadeState::NeedsFirstPlacement;
        }
    }

    /// Placement operation performed while this tile is fading. We hold onto
    /// a tile for two placements: fading starts with the first placement and
    /// will have time to finish by the second placement.
    pub(crate) fn performed_fade_placement(&mut self) {
        match self.fade {
            FadeState::NeedsFirstPlacement => self.fade = FadeState::NeedsSecondPlacement,
            FadeState::NeedsSecondPlacement => self.fade = FadeState::CanRemove,
            FadeState::Loaded | FadeState::CanRemove => {}
        }
    }

    /// Notifies the tile of a visible layer's updated properties. Returns
    /// whether the tile draws the layer.
    pub(crate) fn layer_properties_updated(
        &mut self,
        layer_properties: &Arc<dyn mln_style::layer_properties::LayerProperties>,
    ) -> bool {
        self.contents.as_mut().is_some_and(|contents| Contents::layer_properties_updated(contents, layer_properties))
    }

    pub(crate) fn dump_debug_logs(&self) {
        let kind = match self.kind {
            TileKind::Geometry => "Geometry",
            TileKind::Raster => "Raster",
            TileKind::Dem(_) => "RasterDEM",
        };
        let yes = |value: bool| if value { "yes" } else { "no" };
        Log::info(Event::General, format!("Tile::Kind: {kind}"));
        Log::info(Event::General, format!("Tile::id: {}", self.id));
        Log::info(Event::General, format!("Tile::renderable: {}", yes(self.is_renderable())));
        Log::info(Event::General, format!("Tile::complete: {}", yes(self.is_complete())));
        Log::info(Event::General, format!("Tile::loaded: {}", yes(self.loaded)));
    }
}

impl crate::source_state::FeatureStateTarget for Tile {
    fn feature_state_version(&mut self) -> &mut u64 {
        &mut self.feature_state_version
    }

    fn set_feature_state(&self, states: &crate::bucket::LayerFeatureStates) {
        if let Some(contents) = self.contents.as_deref().and_then(Contents::geometry) {
            contents.set_feature_state(states);
        }
    }
}
