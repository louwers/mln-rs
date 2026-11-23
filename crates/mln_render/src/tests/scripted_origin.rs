//! The tests' source types, `scripted`, `scripted-raster` and
//! `scripted-dem`, whose tiles load what the test sends them. The origin
//! records each hook the engine calls, and hands the test each load it
//! begins as a [`ScriptedLoad`], which the test answers with a [`Reply`].
//!
//! Every tile loads the same way: a required tile without data and without a
//! load begins one; a load stays open for further replies until a failure,
//! a newer load, the tile becoming optional or the tile going stops it; a
//! failed tile loads again when the engine retries it, unless the script
//! says otherwise ([`Retry`]); and a tile that starts from another world
//! copy takes that copy, which counts as data, unless the script refuses
//! copies.

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_storage::{Data, RequestOrder};
use mln_style::Source;
use mln_style::layer_type::SourceKind;
use mln_style::source_type::{SourceData, SourceTypeInfo, StyleSourceType};
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::range::Range;
use mln_util::tileset::RasterEncoding;
use parking_lot::Mutex;

use crate::source::{
    Cover, DataOrigin, LoadContext, OriginUpdate, Replier, SourceType, Supply, TileInput, TileKind, TileNecessity,
    TileOperation,
};

/// The kind of a scripted source's tiles, whose cover picks a view's tiles
/// as the tileset source type of the same kind does.
pub(crate) trait Kind: Send + Sync + 'static {
    const INFO: SourceTypeInfo;
    const TILES: TileKind;
    const ROUND_ZOOM: bool;
    const OVERSCALE: bool;
}

pub(crate) struct Geometry;
pub(crate) struct Raster;
pub(crate) struct Dem;

const fn info(name: &'static str, layer_kind: SourceKind) -> SourceTypeInfo {
    SourceTypeInfo { name, layer_kind, from_json: false }
}

impl Kind for Geometry {
    const INFO: SourceTypeInfo = info("scripted", SourceKind::Geometry);
    const TILES: TileKind = TileKind::Geometry;
    const ROUND_ZOOM: bool = false;
    const OVERSCALE: bool = true;
}

impl Kind for Raster {
    const INFO: SourceTypeInfo = info("scripted-raster", SourceKind::Raster);
    const TILES: TileKind = TileKind::Raster;
    const ROUND_ZOOM: bool = true;
    const OVERSCALE: bool = false;
}

impl Kind for Dem {
    const INFO: SourceTypeInfo = info("scripted-dem", SourceKind::RasterDem);
    const TILES: TileKind = TileKind::Dem(RasterEncoding::Mapbox);
    const ROUND_ZOOM: bool = true;
    const OVERSCALE: bool = true;
}

/// A scripted source type of tiles of the kind `K`.
pub(crate) struct Scripted<K>(PhantomData<fn() -> K>);

/// A scripted source's options: the script its tiles follow, and what they
/// cover.
#[derive(Clone)]
pub(crate) struct ScriptOptions {
    script: Script,
    cover: Cover,
}

impl<K: Kind> StyleSourceType for Scripted<K> {
    const INFO: SourceTypeInfo = K::INFO;

    type Options = ScriptOptions;
    type Description = ();
}

impl<K: Kind> SourceType for Scripted<K> {
    fn supply(source: &Source) -> Supply {
        let options = SourceData::<Self>::recover(source).map(|data| &data.options);
        let origin = ScriptedOrigin {
            script: options.map(|options| options.script.clone()).unwrap_or_default(),
            tiles: K::TILES,
            cover: options.map(|options| options.cover),
            source: source.id().to_owned(),
        };
        Supply::tiles(K::TILES, origin)
    }
}

/// The scripted source `id` of tiles of the kind `K`, `tile_size` pixels
/// wide, of the zoom levels `zoom_range`, which follow `script`.
pub(crate) fn source<K: Kind>(id: &str, script: &Script, tile_size: u16, zoom_range: Range<u8>) -> Source {
    let cover = Cover {
        tile_size,
        zoom_range,
        bounds: None,
        round_zoom: K::ROUND_ZOOM,
        overscale: K::OVERSCALE,
        prefetch: true,
        synchronous: false,
    };
    Source::new(id, SourceData::<Scripted<K>>::new(ScriptOptions { script: script.clone(), cover }))
}

/// A hook the engine called, and the tile it called it for; or what the
/// origin did of its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Call {
    Update,
    Start(OverscaledTileID),
    /// The tile, which started from another world copy, took that copy.
    TookCopy(OverscaledTileID),
    Receive(OverscaledTileID),
    SetNecessity(OverscaledTileID, TileNecessity),
    Retry(OverscaledTileID),
    Failed(OverscaledTileID),
    /// The engine dropped the origin's state of the tile.
    Dropped(OverscaledTileID),
}

/// What a scripted origin does as the engine retries a failed tile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Retry {
    /// Begins a load, which the test answers.
    #[default]
    Load,
    /// Nothing, as an origin without retries.
    Nothing,
    /// Delivers no data at once, without a load.
    Deliver,
}

/// What a test sends a tile's load.
pub(crate) enum Reply {
    /// Encoded data: a vector tile (MVT) for a geometry tile, an image for
    /// a raster or raster-dem one.
    Bytes(Data),
    /// A geometry tile's data.
    Tile(TileData),
    /// No data: the tile is empty.
    Empty,
    /// No tile here: a tile of another zoom level draws in its place.
    Missing,
    /// The load failed with this message.
    Fail(String),
}

/// What the test and its scripted origins share: the hooks the engine
/// called, and the loads that wait for the test's replies.
#[derive(Clone, Default)]
pub(crate) struct Script(Arc<Mutex<ScriptState>>);

#[derive(Default)]
struct ScriptState {
    calls: Vec<Call>,
    loads: Vec<ScriptedLoad>,
    retry: Retry,
    refuses_copies: bool,
}

impl Script {
    /// An origin of tiles of `tiles` that follow the script and cover
    /// nothing, for tiles that a test makes itself.
    pub(crate) fn origin(&self, tiles: TileKind) -> ScriptedOrigin {
        ScriptedOrigin { script: self.clone(), tiles, cover: None, source: String::new() }
    }

    /// Has the origins do what `retry` says as the engine retries a failed
    /// tile.
    pub(crate) fn set_retry(&self, retry: Retry) {
        self.0.lock().retry = retry;
    }

    /// Has the tiles that start from another world copy load anew, rather
    /// than take the copy.
    pub(crate) fn refuse_copies(&self) {
        self.0.lock().refuses_copies = true;
    }

    /// The loads begun since the last call, oldest first.
    pub(crate) fn take_loads(&self) -> Vec<ScriptedLoad> {
        std::mem::take(&mut self.0.lock().loads)
    }

    /// The hooks called since the last call, oldest first.
    pub(crate) fn take_calls(&self) -> Vec<Call> {
        std::mem::take(&mut self.0.lock().calls)
    }

    /// Answers each load begun since the last call with what `reply` makes
    /// of its tile.
    pub(crate) fn answer(&self, mut reply: impl FnMut(&OverscaledTileID) -> Reply) {
        for load in self.take_loads() {
            load.reply(reply(&load.tile));
        }
    }

    fn record(&self, call: Call) {
        self.0.lock().calls.push(call);
    }
}

/// A load that a scripted origin began, which waits for the test's reply.
pub(crate) struct ScriptedLoad {
    /// The source of the tile, as its last update named it.
    pub(crate) source: String,
    pub(crate) tile: OverscaledTileID,
    order: RequestOrder,
    replier: Replier<Reply>,
    stopped: Arc<AtomicBool>,
}

impl ScriptedLoad {
    /// Sends `reply`, which the tile takes in at the next update if the
    /// load is still its latest.
    pub(crate) fn reply(&self, reply: Reply) {
        self.replier.send(reply);
    }

    /// Whether the tile stopped waiting for the load, as an optional tile
    /// or one let go of does.
    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Where the tile's load stands among those that wait, now.
    pub(crate) fn order(&self) -> u32 {
        self.order.get()
    }
}

/// The origin of a scripted source's tiles.
pub(crate) struct ScriptedOrigin {
    script: Script,
    tiles: TileKind,
    cover: Option<Cover>,
    source: String,
}

/// A scripted tile's state.
pub(crate) struct ScriptedTile {
    script: Script,
    id: OverscaledTileID,
    order: RequestOrder,
    /// The flag of the load the tile waits for, which it sets as it stops
    /// waiting.
    open: Option<Arc<AtomicBool>>,
    /// A load brought data, or the tile took its world copy's answer.
    answered: bool,
    /// The tile stopped its latest load, whose replies go nowhere.
    closed: bool,
}

impl ScriptedTile {
    /// Whether the tile has neither data nor a load: a required one is so
    /// only after a failure.
    fn needs_load(&self) -> bool {
        self.open.is_none() && !self.answered
    }

    fn stop(&mut self) {
        if let Some(stopped) = self.open.take() {
            stopped.store(true, Ordering::SeqCst);
        }
    }
}

impl Drop for ScriptedTile {
    fn drop(&mut self) {
        self.stop();
        self.script.record(Call::Dropped(self.id));
    }
}

impl ScriptedOrigin {
    /// Begins a load of `tile`, which the test answers.
    fn begin(&self, tile: &mut ScriptedTile, cx: &mut LoadContext<'_, '_, Reply>) {
        tile.stop();
        tile.closed = false;
        cx.report(TileOperation::RequestedFromNetwork);
        let replier = cx.begin_load();
        let stopped = Arc::new(AtomicBool::new(false));
        tile.open = Some(Arc::clone(&stopped));
        let load =
            ScriptedLoad { source: self.source.clone(), tile: tile.id, order: tile.order.clone(), replier, stopped };
        self.script.0.lock().loads.push(load);
    }
}

impl DataOrigin for ScriptedOrigin {
    type Tile = ScriptedTile;
    type Reply = Reply;

    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        self.script.record(Call::Update);
        update.source().id().clone_into(&mut self.source);
        self.cover
    }

    /// A tile takes its copy, unless the script refuses copies, or waits
    /// until it is required.
    fn start(&mut self, cx: &mut LoadContext<'_, '_, Reply>, copy: Option<&ScriptedTile>) -> ScriptedTile {
        let id = *cx.id();
        self.script.record(Call::Start(id));
        let refuses = self.script.0.lock().refuses_copies;
        let answered = copy.is_some() && !refuses && cx.take_copy();
        if answered {
            self.script.record(Call::TookCopy(id));
        }
        ScriptedTile { script: self.script.clone(), id, order: cx.order(), open: None, answered, closed: false }
    }

    /// A reply after its load stopped goes nowhere.
    fn receive(&mut self, tile: &mut ScriptedTile, reply: Reply, cx: &mut LoadContext<'_, '_, Reply>) {
        if tile.closed {
            return;
        }
        self.script.record(Call::Receive(tile.id));
        let geometry = self.tiles == TileKind::Geometry;
        let (data, image, missing) = match reply {
            Reply::Fail(message) => {
                tile.answered = false;
                tile.stop();
                tile.closed = true;
                cx.fail(Error::native(message));
                return;
            }
            Reply::Bytes(bytes) if geometry => (Some(TileData::Mvt(mln_tile::MvtTile::new(bytes))), None, false),
            Reply::Bytes(image) => (None, Some(image), false),
            Reply::Tile(data) => (Some(data), None, false),
            Reply::Empty => (None, None, false),
            Reply::Missing => (None, None, true),
        };
        tile.answered = true;
        cx.report(TileOperation::LoadFromNetwork);
        cx.deliver(if geometry {
            TileInput::Geometry { data, missing }
        } else {
            TileInput::Image { data: image, missing }
        });
    }

    fn set_necessity(&mut self, tile: &mut ScriptedTile, previous: TileNecessity, cx: &mut LoadContext<'_, '_, Reply>) {
        self.script.record(Call::SetNecessity(tile.id, cx.necessity()));
        if previous == cx.necessity() {
            return;
        }
        match cx.necessity() {
            TileNecessity::Required if tile.needs_load() => self.begin(tile, cx),
            TileNecessity::Required => {}
            TileNecessity::Optional => {
                if tile.open.is_some() {
                    tile.stop();
                    tile.closed = true;
                }
            }
        }
    }

    fn retry(&mut self, tile: &mut ScriptedTile, cx: &mut LoadContext<'_, '_, Reply>) {
        self.script.record(Call::Retry(tile.id));
        if !tile.needs_load() {
            return;
        }
        let retry = self.script.0.lock().retry;
        match retry {
            Retry::Load => self.begin(tile, cx),
            Retry::Nothing => {}
            Retry::Deliver => {
                tile.answered = true;
                cx.deliver(if self.tiles == TileKind::Geometry {
                    TileInput::Geometry { data: None, missing: false }
                } else {
                    TileInput::Image { data: None, missing: false }
                });
            }
        }
    }

    fn failed(&mut self, tile: &mut ScriptedTile, _cx: &mut LoadContext<'_, '_, Reply>) {
        self.script.record(Call::Failed(tile.id));
    }
}
