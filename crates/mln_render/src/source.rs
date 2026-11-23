//! The source type contract: what a crate implements to add a source type
//! whose sources the renderer draws. The `mln_extension_tests` crate's
//! `stamp` source type, whose tiles each hold a value that its origin makes
//! from the tile's id, is a whole example written against the facade alone,
//! whose `mln_rs::source` gathers every name it uses; the built-in source
//! types of `mln_sources` go through the same contract, and its tileset
//! origin is the example of one that loads through the map's file source.
//!
//! A source type has two halves, which
//! [`RegistryBuilder::add_source_type::<T>()`](crate::RegistryBuilder::add_source_type)
//! registers together. It fails for a name that another type took, and for
//! a type whose halves name different kinds of data.
//!
//! - **The style half**, a marker type that implements `mln_style`'s
//!   [`StyleSourceType`]: the type's name and its kind of data
//!   ([`SourceTypeInfo`](mln_style::source_type::SourceTypeInfo)), the
//!   options it reads from style JSON, and how the map loads what the style
//!   doesn't hold itself, such as a TileJSON or GeoJSON data, into the
//!   source's description. stamp's is `StampSource`, with its `version` and
//!   `hold` members.
//! - **The render half**, [`SourceType`] on the same marker type: what a
//!   source of the type supplies the renderer.
//!
//! # What a source type writes
//!
//! 1. Its kind of data, the [`Data`](SourceType::Data) that its sources
//!    supply: tiles of [geometry](GeometryTiles), which the engine lays out
//!    into the buckets of the layers that draw them, of
//!    [raster images](RasterTiles) or of [raster-dem elevations](DemTiles),
//!    which it decodes, or of a [value of the type's own](ValueTiles),
//!    which the layers that view it share as it is. A new kind of data that
//!    the engine lays out or decodes for any layer is a change to the
//!    engine, not a source type.
//! 2. Its [supply](SourceType::supply), made once as a source joins the
//!    renderer's style: tiles that a [`DataOrigin`] loads
//!    ([`Supply::tiles`]), or, for raster data, one image at four
//!    coordinates that the engine reads from each version of the source
//!    ([`Supply::image`]), as the image source type supplies.
//! 3. Its origin. [`DataOrigin::update`] takes in the source as the style
//!    has it and returns what its tiles [`Cover`]: their size, zoom levels
//!    and bounds, and how the engine picks a view's tiles of them.
//!    [`DataOrigin::start`] starts each new tile and returns the origin's
//!    state of it, its [`Tile`](DataOrigin::Tile), which the engine keeps
//!    with the tile and drops as the tile goes, which lets go of what it
//!    holds. Through the tile's [`LoadContext`] the origin
//!    [begins a load](LoadContext::begin_load), whose [`Replier`] sends what
//!    the load gets back from any thread to [`DataOrigin::receive`], and
//!    [delivers](LoadContext::deliver) what it brought: an
//!    [item](TileKind::Item) of the source's kind, nothing for an empty
//!    tile, or [missing](LoadContext::missing) for a tile the source doesn't
//!    have. stamp's origin delivers each tile's value in `start`, with no
//!    load. An origin that loads cheaply first, or retries, or keeps what
//!    it loaded fresh, also writes [`set_necessity`](DataOrigin::set_necessity),
//!    [`retry`](DataOrigin::retry) and [`failed`](DataOrigin::failed), as
//!    the tileset's does.
//! 4. Its commands, if an app edits or reads its sources outside the
//!    style JSON. They go through the map's typed source paths
//!    (`Map::command_style_source`, `Map::query_style_source`,
//!    `Map::set_style_source_data`, with `Map::apply_style_edit` for an
//!    [`Edit`](mln_style::Edit)), as `mln_sources`' GeoJSON and custom
//!    commands do.
//!
//! # When the hooks run
//!
//! The engine runs [`SourceType::supply`] and every hook of the origin one
//! at a time on the renderer's thread, never one from within another, so an
//! origin and its tiles' states needn't be `Send`. In each of the
//! renderer's updates, it first hands each tile the replies that arrived
//! for its latest load ([`receive`](DataOrigin::receive)), then, per
//! source, runs [`update`](DataOrigin::update) with the source as the style
//! has it, the same version until the style changes it, so the origin tells
//! a new version from the last it took in and then clears or loads its
//! tiles again ([`OriginUpdate`]). With the cover `update` returns, the
//! engine works out the view's tiles: it [starts](DataOrigin::start) those
//! it makes, and sets the [necessity](DataOrigin::set_necessity) of each it
//! keeps, as it first keeps it in the update and as it lets it go into its
//! cache. The engine runs [`retry`](DataOrigin::retry) on required tiles
//! when the map retries failed loads, and before each still image, and
//! [`failed`](DataOrigin::failed) when what a tile took in fails to lay out
//! or decode.
//!
//! A load's work runs wherever the origin puts it: on the renderer's pool
//! ([`LoadContext::pool`]), which runs on the renderer's thread in the
//! browser, on a file source's threads, or in a host's callback. Its
//! [`Replier`] and its replies are `Send`, and a replier sends as many
//! replies as its load has. What the tiles take in lays out or decodes on
//! the tile workers, through the worker codec where they run apart, or in
//! the update that delivers it for a [synchronous](Cover::synchronous)
//! cover; an origin's own types and values never cross the codec.
//!
//! # What the engine owns
//!
//! The engine owns how MapLibre loads tiles; the origin owns where their
//! data comes from. The engine decides which tiles the view needs, in which
//! order their requests start ([`LoadContext::order`]), and which it keeps
//! in its cache and evicts. While a tile has nothing to show, it draws the
//! tile's children or parent that can draw; it makes a missing parent
//! only once the tile has loaded, or has [tried](LoadContext::tried_optional)
//! what it can while optional, so a parent never loads for a tile that
//! loads at once. It makes a tile in a new world copy show the same tile's
//! contents in another, which [`WorldCopy::take`] makes the new tile's
//! answer, and hands a reply to the origin only while its load is the
//! tile's latest. It lays out and decodes what tiles take in, knows
//! when a tile has done what it can, and starts a failed tile over when its
//! retry loads anew. How data is requested, cached, revalidated, kept fresh
//! and retried is the origin's.
//!
//! # Necessity
//!
//! A tile is required while the view needs it, and optional while it is
//! only a fallback or a prefetch the view can do without, or waits in the
//! cache. Necessity is a hint: an origin with a cheap way to get data, such
//! as a cache, may try only that while the tile is optional, and say when
//! it did ([`LoadContext::tried_optional`]), so that a parent can stand in;
//! it asks for more as the tile becomes required, and stops as it becomes
//! optional, as the tileset's origin does with its network requests, and
//! the custom source's with its host's. An origin without one loads every
//! tile as it starts and ignores necessity.
//!
//! # Tile data of a source type's own
//!
//! A source type whose data only its own layer types draw supplies
//! [`ValueTiles<V>`]: its origin delivers an `Arc<V>` for each tile, which
//! it makes once, wherever it likes, and the layers that view the source
//! share it as it is, each tile's `&V`, with no layout and no bucket. Both
//! halves of the source type name the kind as
//! [`SourceKind::Value`](mln_style::layer_type::SourceKind::Value) of
//! [`ValueKind::of::<V>()`](mln_style::layer_type::ValueKind::of), as the
//! style halves of the layer types that view it do, with the input
//! [`crate::layer::ValueTiles`], so that a style whose layer views a source
//! of another kind, or of values of another type, leaves the layer out
//! with an error. A tile shows the value of its latest delivery at once, a
//! parent or children draw for a tile without one, a tile in a new world
//! copy shares its copy's value, and the value goes with its tile. A value
//! doesn't change once delivered: a tile that needs another gets a new one,
//! from which the layers make their tile values again. Tiles overlap by a
//! parent drawn under its loading children, so a layer clips its draws to
//! its tile ([`Draw::clip`](crate::draw::Draw::clip)). stamp's `Stamp` is
//! such a value, and its `stamp-fill` and `stamp-mark` layer types both
//! draw from it.
use std::marker::PhantomData;
use std::sync::Arc;

use mln_map_types::MapMode;
use mln_storage::{Data, FileSource, RequestOrder};
use mln_style::Source;
use mln_style::source_type::StyleSourceType;
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::LatLng;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::range::Range;
use mln_util::tileset::RasterEncoding;

use crate::erased_source::{ErasedOrigin, UpdateParts, typed};
pub use crate::layer::{DemTiles, RasterTiles, ValueTiles};
use crate::tile::inbox::{InboxSender, TileEvent};
use crate::tile::job::TileKey;
pub use crate::tile::loading::TileTimes;
pub use crate::tile::tile::{TileNecessity, TileOperation};
use crate::tile::tile_mut::{CopyAnswer, TileMut};

/// The render half of a source type, whose style half is its
/// [`StyleSourceType`]: what its sources supply the renderer.
pub trait SourceType: StyleSourceType {
    /// The kind of data its sources supply: the kind its style half's
    /// [`layer_kind`](mln_style::source_type::SourceTypeInfo::layer_kind)
    /// names, or the type doesn't register.
    type Data: TileKind;

    /// What `source`, a source of the type, supplies the renderer, which
    /// makes it once, as the source joins the renderer's style.
    fn supply(source: &Source) -> Supply<Self::Data>;
}

/// What a source supplies the renderer: tiles of the kind `K`, which a data
/// origin loads, or one image at four coordinates.
pub struct Supply<K>(pub(crate) Supplied, PhantomData<fn() -> K>);

pub(crate) enum Supplied {
    Tiles(Box<dyn ErasedOrigin>),
    Image(ImageOf),
}

/// The image of a source, if it is one of the type's.
pub type ImageOf = for<'a> fn(&'a Source) -> Option<SourceImage<'a>>;

impl<K: TileKind> Supply<K> {
    /// Tiles whose data comes from `origin`.
    #[must_use]
    pub fn tiles<O: DataOrigin<Data = K>>(origin: O) -> Self {
        Supply(Supplied::Tiles(Box::new(origin)), PhantomData)
    }
}

impl Supply<RasterTiles> {
    /// One image, which `image_of` reads from each version of the source.
    #[must_use]
    pub fn image(image_of: ImageOf) -> Self {
        Supply(Supplied::Image(image_of), PhantomData)
    }
}

/// A kind of tile data, and what a tile's load delivers of it, its
/// [`Item`](Self::Item). The kinds are the engine's: [`GeometryTiles`],
/// [`RasterTiles`], [`DemTiles`] and [`ValueTiles`].
#[expect(private_bounds, reason = "the engine's kinds are the only ones: `kind::Deliver` seals the trait")]
pub trait TileKind: kind::Deliver + 'static {
    /// What a load delivers to a tile of the kind.
    type Item;
}

/// The tiles of a geometry source, whose features the engine lays out into
/// the buckets of the layers that draw them. A load delivers a tile's
/// [`TileData`].
pub enum GeometryTiles {}

impl TileKind for GeometryTiles {
    type Item = TileData;
}

/// A load delivers a raster tile's encoded image, which the engine decodes.
impl TileKind for RasterTiles {
    type Item = Data;
}

/// A load delivers a raster-dem tile's encoded image and how its pixels
/// encode elevations, which the engine decodes.
impl TileKind for DemTiles {
    type Item = (Data, RasterEncoding);
}

/// A load delivers a tile's value, which the layers that view it share as
/// it is, and which the tile's world copies share too.
impl<V: Send + Sync + 'static> TileKind for ValueTiles<V> {
    type Item = Arc<V>;
}

pub(crate) mod kind {
    use std::sync::Arc;

    use mln_style::layer_type::{SourceKind, ValueKind};

    use super::{DemTiles, GeometryTiles, RasterTiles, TileKind, ValueTiles};
    use crate::tile::tile_mut::TileMut;

    /// The engine's side of a [`TileKind`]: the kind of source it is, and
    /// how a tile takes in its items. It seals [`TileKind`].
    pub(crate) trait Deliver {
        const KIND: SourceKind;

        /// Gives `tile` what a load delivered: `None` for an empty tile.
        fn deliver(tile: &mut TileMut<'_>, item: Option<<Self as TileKind>::Item>)
        where
            Self: TileKind;
    }

    impl Deliver for GeometryTiles {
        const KIND: SourceKind = SourceKind::Geometry;

        fn deliver(tile: &mut TileMut<'_>, item: Option<<Self as TileKind>::Item>) {
            tile.take_data(item, false);
        }
    }

    impl Deliver for RasterTiles {
        const KIND: SourceKind = SourceKind::Raster;

        fn deliver(tile: &mut TileMut<'_>, item: Option<<Self as TileKind>::Item>) {
            tile.take_image(item.map(|data| (data, None)), false);
        }
    }

    impl Deliver for DemTiles {
        const KIND: SourceKind = SourceKind::RasterDem;

        fn deliver(tile: &mut TileMut<'_>, item: Option<<Self as TileKind>::Item>) {
            tile.take_image(item.map(|(data, encoding)| (data, Some(encoding))), false);
        }
    }

    impl<V: Send + Sync + 'static> Deliver for ValueTiles<V> {
        const KIND: SourceKind = SourceKind::Value(ValueKind::of::<V>());

        fn deliver(tile: &mut TileMut<'_>, item: Option<<Self as TileKind>::Item>) {
            tile.take_value(item.map(|value| -> Arc<dyn std::any::Any + Send + Sync> { value }));
        }
    }
}

/// An image at coordinates: what an image source shows.
#[derive(Clone, Copy, Debug)]
pub struct SourceImage<'a> {
    /// The image, `None` until it arrives or for a source without one.
    pub image: Option<&'a Arc<PremultipliedImage>>,
    /// The image's corners: top left, top right, bottom right, bottom left.
    pub coordinates: [LatLng; 4],
}

/// What a source's tiles cover, and how the engine picks those of a view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cover {
    /// The size of the tiles, in pixels.
    pub tile_size: u16,
    /// The zoom levels that have tiles of their own.
    pub zoom_range: Range<u8>,
    /// The bounds outside which there are no tiles.
    pub bounds: Option<LatLngBounds>,
    /// Whether a view's tiles are of the zoom level nearest the view's, as
    /// raster tiles are, rather than of the level below it. A render in tile
    /// mode then draws every tile of the view, not only the first.
    pub round_zoom: bool,
    /// Whether a view past the last zoom level of `zoom_range` asks for
    /// tiles of its own zoom, made from that level's, rather than drawing
    /// that level's tiles larger.
    pub overscale: bool,
    /// Whether, in continuous mode, tiles a few zoom levels below the
    /// view's load first, and the globe's lowest level stays loaded, so that
    /// something shows while the view's tiles load.
    pub prefetch: bool,
    /// Whether the tiles lay out on the renderer's thread in the update that
    /// gives them their data, rather than on the tile workers.
    pub synchronous: bool,
}

/// Where a source's tiles get their data: what each tile has of the
/// source's data, and how it loads.
///
/// The tile hooks ([`start`](Self::start), [`receive`](Self::receive),
/// [`set_necessity`](Self::set_necessity), [`retry`](Self::retry) and
/// [`failed`](Self::failed)) get the tile's [`LoadContext`], and all but
/// `start` the origin's state of the tile. The engine calls none of the
/// hooks from within another.
pub trait DataOrigin: 'static {
    /// The kind of data its tiles take in.
    type Data: TileKind;

    /// What the origin keeps of each tile. The engine drops it as the tile
    /// goes, which lets go of what it holds, such as a request or a
    /// subscription.
    type Tile: 'static;

    /// What a tile's load gets back, which [`receive`](Self::receive)
    /// takes in.
    type Reply: Send + 'static;

    /// Takes in the source as the style has it now, before each update of
    /// the cover, which is in each of the renderer's updates: the same
    /// version until the style changes it, so the origin tells a new
    /// version from the last it took in, and then clears or loads the tiles
    /// again as the source's rules say. Returns what the tiles cover, or
    /// `None` while the source has nothing to make tiles from, in which case
    /// the engine leaves its tiles as they are and makes no new ones.
    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover>;

    /// Starts a new tile and returns the origin's state of it. The tile
    /// starts optional; the cover sets how much it needs it
    /// ([`set_necessity`](Self::set_necessity)) as it retains it, after
    /// this. On a flat map, `copy` is the same tile in another world copy,
    /// whose contents the new tile already shows, and
    /// [`copy.take(cx)`](WorldCopy::take) settles the tile on them, as its
    /// load's answer. Otherwise the origin delivers the tile's data at once,
    /// starts a load, or waits until the tile is needed.
    fn start(&mut self, cx: &mut LoadContext<'_, '_, Self>, copy: Option<WorldCopy<'_, Self::Tile>>) -> Self::Tile;

    /// Takes in `reply`, which came for the tile's latest load.
    fn receive(&mut self, tile: &mut Self::Tile, reply: Self::Reply, cx: &mut LoadContext<'_, '_, Self>);

    /// The engine set how much the tile is needed, to
    /// [`cx.necessity()`](LoadContext::necessity) from `previous`, which may
    /// be the same: as each update's cover retains the tile, and as it
    /// becomes optional.
    fn set_necessity(&mut self, _tile: &mut Self::Tile, _previous: TileNecessity, _cx: &mut LoadContext<'_, '_, Self>) {
    }

    /// The engine retries the required tile's failed loads: in continuous
    /// mode when the map is told to, and before each still image. A tile
    /// that this begins a load of, or gives new data, loads anew; one that
    /// it leaves as it was stays as it was.
    fn retry(&mut self, _tile: &mut Self::Tile, _cx: &mut LoadContext<'_, '_, Self>) {}

    /// The engine couldn't lay out or decode what the tile last took in, and
    /// reported the error; the tile keeps what it shows. Not called for the
    /// origin's own [`LoadContext::fail`], nor for a layout that runs, and
    /// fails, within the hook that delivered, as a
    /// [synchronous](Cover::synchronous) cover's does.
    fn failed(&mut self, _tile: &mut Self::Tile, _cx: &mut LoadContext<'_, '_, Self>) {}
}

/// A [`DataOrigin`]'s update: the source's new version, what the map says
/// of its tiles, and the tiles, which the origin can clear or load again.
pub struct OriginUpdate<'a, O: ?Sized> {
    pub(crate) parts: UpdateParts<'a>,
    pub(crate) origin: PhantomData<fn(&mut O)>,
}

impl<'a, O: DataOrigin> OriginUpdate<'a, O> {
    /// The source as the style has it now.
    #[must_use]
    pub fn source(&self) -> &'a Source {
        self.parts.source
    }

    /// Whether the map draws continuously or still images, which a tile
    /// shows only once it has done what it can.
    #[must_use]
    pub fn mode(&self) -> MapMode {
        self.parts.parameters.mode
    }

    /// The map's pixel ratio, which a raster tile's resource may ask for.
    #[must_use]
    pub fn pixel_ratio(&self) -> f32 {
        self.parts.parameters.pixel_ratio
    }

    /// The map's highest zoom level.
    #[must_use]
    pub fn max_zoom(&self) -> f64 {
        self.parts.parameters.max_zoom
    }

    /// Where the map's resources load from, if anywhere.
    #[must_use]
    pub fn file_source(&self) -> Option<&Arc<dyn FileSource>> {
        self.parts.parameters.file_source.as_ref()
    }

    /// Whether the source's tiles lay out again.
    #[must_use]
    pub fn needs_relayout(&self) -> bool {
        self.parts.needs_relayout
    }

    /// Whether MLT tiles decode FastPFOR-compressed streams.
    #[must_use]
    pub fn fast_pfor_enabled(&self) -> bool {
        self.parts.parameters.fast_pfor_enabled
    }

    /// Lets go of every tile.
    pub fn clear_tiles(&mut self) {
        self.parts.tiles.clear_all(self.parts.ctx);
    }

    /// Lets go of the cached tiles, those the view no longer needs.
    pub fn clear_cache(&mut self) {
        self.parts.tiles.cache_clear(self.parts.ctx);
    }

    /// Changes each tile of the cover, not of the cache, that `filter`
    /// picks, through `f`, which gets the origin's state of it.
    pub fn update_tiles(
        &mut self,
        filter: impl Fn(&OverscaledTileID) -> bool,
        mut f: impl FnMut(&mut O::Tile, &mut LoadContext<'_, '_, O>),
    ) {
        self.parts.tiles.update_each(
            self.parts.ctx,
            |slot| !slot.place().is_cached() && filter(&slot.tile.id),
            |tile| {
                tile.with_state(|state, tile| {
                    if let Some(state) = typed::<O>(state) {
                        f(state, &mut LoadContext::new(tile));
                    }
                });
            },
        );
    }
}

/// A tile, as a [`DataOrigin`]'s hook sees it: what the engine knows of it,
/// and what the origin can do to it.
pub struct LoadContext<'a, 't, O: ?Sized> {
    tile: &'a mut TileMut<'t>,
    origin: PhantomData<fn(&O)>,
}

impl<'a, 't, O: DataOrigin + ?Sized> LoadContext<'a, 't, O> {
    pub(crate) fn new(tile: &'a mut TileMut<'t>) -> Self {
        LoadContext { tile, origin: PhantomData }
    }

    /// The tile's id, with its world copy and the zoom it draws at.
    #[must_use]
    pub fn id(&self) -> &OverscaledTileID {
        &self.tile.tile().id
    }

    /// How much the view needs the tile now: a hint, which an origin
    /// without a cheaper way to get data ignores.
    #[must_use]
    pub fn necessity(&self) -> TileNecessity {
        self.tile.tile().load.necessity
    }

    /// When the tile's requests start among those that wait for a
    /// connection, which the engine keeps up to date: for the tile's
    /// resources.
    #[must_use]
    pub fn order(&self) -> RequestOrder {
        self.tile.tile().load.order.clone()
    }

    /// Sets when the tile's data was modified and when it expires, which
    /// the tile debug overlay shows.
    pub fn set_times(&mut self, times: TileTimes) {
        self.tile.tile_mut().load.times = times;
    }

    /// Whether the tile can draw, if only some of its layers.
    #[must_use]
    pub fn is_renderable(&self) -> bool {
        self.tile.tile().is_renderable()
    }

    /// The renderer's thread pool.
    #[must_use]
    pub fn pool(&self) -> &Pool {
        self.tile.store().pool.pool()
    }

    /// Starts a load, the tile's latest from now on, whose replies the
    /// returned replier sends. Replies to the loads before go nowhere. A
    /// load stays the latest until the next one begins, so an origin that
    /// stops a load's work ignores the replies it sent before it stopped.
    pub fn begin_load(&mut self) -> Replier<O::Reply> {
        let load = self.tile.next();
        self.tile.tile_mut().load.current = Some(load);
        let tile = self.tile.key();
        Replier { inbox: self.tile.store().inbox.clone(), tile, load, reply: PhantomData }
    }

    /// Gives the tile what a load brought, which it lays out or decodes, or
    /// shows at once if it is a value: `None` for an empty tile, which
    /// shows nothing.
    pub fn deliver(&mut self, item: Option<<O::Data as TileKind>::Item>) {
        <O::Data as kind::Deliver>::deliver(self.tile, item);
    }

    /// A load found that the source has no tile here: a tile of another
    /// zoom level draws in its place.
    pub fn missing(&mut self) {
        self.tile.take_missing();
    }

    /// The tile couldn't load: it keeps what it shows, and the engine
    /// reports `error`.
    pub fn fail(&mut self, error: Error) {
        self.tile.fail(error);
    }

    /// A load found the tile's data unchanged: the tile has done what it
    /// can, unless work on its data is under way, which settles it as it
    /// ends.
    pub fn keep_data(&mut self) {
        self.tile.keep_data();
    }

    /// The tile has done what it can while optional, such as looking in a
    /// cache. From then on, while it has nothing to show, the engine makes
    /// its parent if the cover lacks one, and loads it to stand in; for a
    /// tile that hasn't said so, only a parent the cover has stands in.
    pub fn tried_optional(&mut self) {
        self.tile.tile_mut().load.tried_optional = true;
    }

    /// Reports what the tile's load did: requested from the cache or the
    /// network, or loaded from either. The engine reports the rest of a
    /// tile's operations itself.
    pub fn report(&mut self, op: TileOperation) {
        debug_assert!(
            matches!(
                op,
                TileOperation::RequestedFromCache
                    | TileOperation::RequestedFromNetwork
                    | TileOperation::LoadFromCache
                    | TileOperation::LoadFromNetwork
            ),
            "an origin reports its loads' requests and answers, not {op:?}"
        );
        self.tile.action(op);
    }

    /// The tile changed: the renderer needs a frame.
    pub fn changed(&mut self) {
        self.tile.store_mut().changed();
    }
}

impl<O: DataOrigin<Data = GeometryTiles> + ?Sized> LoadContext<'_, '_, O> {
    /// Drops the tile's data and style, so that it lays out anew once it
    /// has both again.
    pub fn reset(&mut self) {
        self.tile.reset();
    }
}

/// The same tile in another world copy, from whose contents a new tile
/// starts ([`DataOrigin::start`]): the origin's state of it, and what
/// taking it settles the new tile on.
pub struct WorldCopy<'a, T> {
    pub(crate) state: &'a T,
    pub(crate) answer: Option<CopyAnswer>,
}

impl<'a, T> WorldCopy<'a, T> {
    /// The origin's state of the copy.
    #[must_use]
    pub fn state(&self) -> &'a T {
        self.state
    }

    /// Settles the new tile on the copy's contents, which it shows, as its
    /// load's answer: a geometry tile lays out the copy's data for itself,
    /// and a raster, raster-dem or value tile keeps what it shows. Returns
    /// false, and changes nothing, if the copy has no data to give, as one
    /// whose decode is still pending.
    pub fn take<O: DataOrigin<Tile = T> + ?Sized>(self, cx: &mut LoadContext<'_, '_, O>) -> bool {
        let Some(answer) = self.answer else {
            return false;
        };
        cx.tile.take_copy(answer);
        true
    }
}

/// Sends the replies of one of a tile's loads to the tile, from any thread.
pub struct Replier<R> {
    inbox: InboxSender,
    tile: TileKey,
    load: u64,
    reply: PhantomData<fn(R)>,
}

impl<R> Clone for Replier<R> {
    fn clone(&self) -> Self {
        Replier { inbox: self.inbox.clone(), tile: self.tile, load: self.load, reply: PhantomData }
    }
}

impl<R: Send + 'static> Replier<R> {
    /// Sends `reply`, which the tile takes in at the renderer's next update
    /// if the load is still its latest.
    pub fn send(&self, reply: R) {
        self.inbox.send(TileEvent::Load { tile: self.tile, load: self.load, reply: Box::new(reply) });
    }
}
