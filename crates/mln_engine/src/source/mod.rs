//! The source type contract: what a crate implements to add a source type
//! whose sources the renderer draws. `mln_builtins` implements it for the
//! style specification's source types.
//!
//! A source type has two halves, which
//! [`RegistryBuilder::add_source_type::<T>()`](crate::RegistryBuilder::add_source_type)
//! registers together. It fails for a name that another type took, and for
//! a type whose halves name different kinds of data.
//!
//! - **The style half**, a marker type that implements `mln_style`'s
//!   [`StyleSourceType`]: the type's name and its kind of data
//!   ([`SourceTypeInfo`](mln_style::source::SourceTypeInfo)), and the options
//!   it reads from style JSON. It may be another crate's, as the
//!   specification's types are `mln_style`'s, so that a tool that only
//!   parses styles needs no engine.
//! - **The engine half**, [`SourceType`], whose [`Style`](SourceType::Style)
//!   names the style half: how the map loads what the style doesn't hold
//!   itself, such as a TileJSON or GeoJSON data, into the source's
//!   description, what a source reports about itself and offline downloads
//!   fetch, and what it supplies the renderer.
//!
//! # What a source type writes
//!
//! 1. How its description loads, if it has one: [`SourceType::load`] says
//!    what loads for a source's options, which the map's loader fetches and
//!    hands to [`SourceType::describe`] or [`SourceType::describe_image`].
//!    The style holds the description without knowing its type, in the
//!    source's [`SourceData::description`]; the type reads it back with
//!    [`SourceData::description`]`::<Self::Description>()`.
//! 2. Its kind of data, the [`Data`](SourceType::Data) that its sources
//!    supply: tiles of [geometry](GeometryTiles), which the engine lays out
//!    into the buckets of the layers that draw them, or of
//!    [raster images](RasterTiles) or [raster-dem elevations](DemTiles),
//!    which it decodes. A new kind of data is a change to the engine, not a
//!    source type.
//! 3. Its [supply](SourceType::supply), made once as a source joins the
//!    renderer's style: tiles that a [`DataOrigin`] loads
//!    ([`Supply::tiles`]), or, for raster data, one image at four
//!    coordinates that the engine reads from each version of the source
//!    ([`Supply::image`]), as the image source type supplies.
//! 4. Its origin. [`DataOrigin::update`] takes in the source as the style
//!    has it and returns what its tiles [`Cover`]. [`DataOrigin::start`]
//!    starts each new tile and returns the origin's state of it, its
//!    [`Tile`](DataOrigin::Tile), which the engine keeps with the tile and
//!    drops as the tile goes, which lets go of what it holds. Through the
//!    tile's [`LoadContext`] the origin [begins a load](LoadContext::begin_load),
//!    whose [`Replier`] sends what the load gets back from any thread to
//!    [`DataOrigin::receive`], and [delivers](LoadContext::deliver) what it
//!    brought: an [item](TileKind::Item) of the source's kind, nothing for an
//!    empty tile, or [missing](LoadContext::missing) for a tile the source
//!    doesn't have. An origin that loads cheaply first, or retries, or keeps
//!    what it loaded fresh, also writes
//!    [`set_necessity`](DataOrigin::set_necessity),
//!    [`retry`](DataOrigin::retry) and [`failed`](DataOrigin::failed), as the
//!    tileset's does.
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
//! cache. The engine runs [`retry`](DataOrigin::retry) on each required
//! tile of the cover before each still image, and
//! [`failed`](DataOrigin::failed) when what a tile took in fails to lay out
//! or decode.
//!
//! A load's work runs wherever the origin puts it: on the renderer's pool
//! ([`LoadContext::pool`]), which runs on the renderer's thread in the
//! browser, on a file source's threads, or in a host's callback. Its
//! [`Replier`] and its replies are `Send`, and a replier sends as many
//! replies as its load has. What the tiles take in lays out or decodes on
//! the tile workers, through the worker codec where they run apart, or, for
//! geometry of a source that updates synchronously, lays out in the update
//! that delivers it; an origin's own types and values never cross the
//! codec.
//!
//! # What the engine owns
//!
//! The engine owns how MapLibre loads tiles; the origin owns where their
//! data comes from. The engine decides which tiles the view needs, in which
//! order their requests start ([`LoadContext::order`]), and which it keeps
//! in its cache and evicts: a tile with nothing to show whose origin said
//! its load [failed and waits for a retry](LoadContext::fail_retrying)
//! isn't loading, so the cache lets go of it, and a cached tile whose data
//! its origin said may no longer be shown ([`TileTimes::usable_until`])
//! goes rather than come back. While a tile has nothing to show, it draws
//! the tile's children or parent that can draw; it makes a missing parent
//! only once the tile has loaded, or has
//! [tried](LoadContext::tried_optional) what it can while optional, so a
//! parent never loads for a tile that loads at once. It makes a tile in a
//! new world copy show the same tile's contents in another, which
//! [`WorldCopy::take`] makes the new tile's answer, and hands a reply to
//! the origin only while its load is the tile's latest. It lays out and
//! decodes what tiles take in, knows when a tile has done what it can, and
//! starts a failed tile over when its retry loads anew. How data is
//! requested, cached, revalidated, kept fresh and retried is the origin's.
//!
//! # Necessity
//!
//! A tile is required while the view needs it, and optional while it is
//! only a fallback or a prefetch the view can do without, or waits in the
//! cache. Necessity is a hint: an origin with a cheap way to get data, such
//! as a cache, may try only that while the tile is optional, and say when
//! it did ([`LoadContext::tried_optional`]), so that the engine makes and
//! loads a parent to stand in; it asks for more as the tile becomes
//! required, and stops as it becomes optional, as the tileset's origin does
//! with its network requests. An origin without one loads every tile as it
//! starts and ignores necessity: a parent draws for its loading tiles only
//! if the cover already has it.

use std::marker::PhantomData;
use std::sync::Arc;

use mln_style::Source;
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::geo::{LatLng, LatLngBounds};
use mln_style::range::Range;
use mln_style::source::{SourceData, StyleSourceType};
use mln_style::tileset::{RasterEncoding, Tileset};

use crate::exec::pool::Pool;
use crate::map::MapMode;
use crate::query::FeatureExtensionValue;
use crate::source::erased::{ErasedOrigin, UpdateParts, typed};
use crate::storage::resource::RequestOrder;
use crate::storage::{Data, FileSource};
use crate::tile::{OverscaledTileID, TileData};
use crate::tile_store::inbox::{InboxSender, TileEvent};
pub use crate::tile_store::loading::TileTimes;
pub use crate::tile_store::tile::{TileNecessity, TileOperation};
use crate::tile_store::tile_mut::{CopyAnswer, TileMut};
use crate::worker::job::TileKey;

pub(crate) mod erased;
mod loading;
pub(crate) mod render_image_source;
pub(crate) mod render_source;
pub(crate) mod render_tile_source;
pub mod source_state;

pub use loading::{Described, Fetch, Load, OfflineResources, Respond, SourceSummary};

/// The options of a source of the type `T`.
pub type OptionsOf<T> = <<T as SourceType>::Style as StyleSourceType>::Options;

/// A source type's engine half: how its sources load, what they report and
/// what they supply the renderer.
///
/// The hooks that take a source's data take it as the style has it, a
/// [`SourceData`] of the style half.
pub trait SourceType: Send + Sync + 'static {
    /// The style half: the type's name, its kind of data and its options.
    type Style: StyleSourceType;

    /// The kind of data its sources supply: the kind its style half's
    /// [`layer_kind`](mln_style::source::SourceTypeInfo::layer_kind) names,
    /// or the type doesn't register.
    type Data: TileKind;

    /// What loading a source finds: a tileset, GeoJSON data, an image. The
    /// style holds it in [`SourceData::description`].
    type Description: Send + Sync + 'static;

    /// Whether [`describe`](Self::describe) runs on the thread pool rather
    /// than on the map's thread, for work such as indexing GeoJSON.
    const DESCRIBES_IN_BACKGROUND: bool = false;

    /// How `source`'s description loads for its current options. The map
    /// asks again whenever [`SourceData::load`] grows.
    fn load(_source: &SourceData<Self::Style>) -> Load<Self::Description> {
        Load::Done
    }

    /// Turns `data`, the fetched resource of a [`Fetch::Source`], into the
    /// description of a source of `options`, and answers through `respond`,
    /// at once or later. `data` is `None` for a response without content,
    /// and for a [`Load::Describe`], which describes the options alone.
    fn describe(_options: &OptionsOf<Self>, _data: Option<Data>, respond: Respond<Described<Self::Description>>) {
        respond(Err(Error::native(format!("a {} source fetches nothing", Self::Style::INFO.name))));
    }

    /// Turns `image`, the decoded image of a [`Fetch::Image`], into the
    /// description of a source of `options`.
    fn describe_image(_options: &OptionsOf<Self>, _image: PremultipliedImage) -> Described<Self::Description> {
        Err(Error::native(format!("a {} source fetches no image", Self::Style::INFO.name)))
    }

    /// Whether `new`, a loaded description that replaces `old`, changes what
    /// the source says about itself, such as a tileset's tiles and
    /// attribution, which the map reports as a change of the source.
    #[must_use]
    fn metadata_changed(_old: Option<&Self::Description>, _new: Option<&Self::Description>) -> bool {
        false
    }

    /// The source's attribution, as its style or its description gives it.
    fn attribution(_source: &SourceData<Self::Style>) -> Option<String> {
        None
    }

    /// What the source reports about itself.
    fn summary(_source: &SourceData<Self::Style>) -> SourceSummary {
        SourceSummary::default()
    }

    /// What an offline download fetches for the source.
    fn offline(_source: &SourceData<Self::Style>) -> OfflineResources {
        OfflineResources::None
    }

    /// The tileset that an offline download takes the source's tiles from,
    /// once the TileJSON that [`OfflineResources::TileJson`] named arrived.
    ///
    /// # Errors
    ///
    /// When `tilejson` doesn't give the source a tileset.
    fn offline_tileset(_source: &SourceData<Self::Style>, _tilejson: &[u8]) -> Result<Tileset, String> {
        Err(format!("a {} source has no TileJSON", Self::Style::INFO.name))
    }

    /// What `source`, a source of the type, supplies the renderer, which
    /// makes it once, as the source joins the renderer's style. A source
    /// that later takes its place with the same id and type, as a style
    /// reload makes, keeps the supply: its origin takes the new source in
    /// [`DataOrigin::update`], and an image's [`ImageOf`] reads it.
    fn supply(source: &Source) -> Supply<Self::Data>;

    /// The `field` of the extension `extension` of `feature`, a feature
    /// that a query found in a tile of `source`, with `args`: such as a
    /// GeoJSON cluster's leaves. A null value for an extension the type
    /// doesn't have.
    ///
    /// # Errors
    ///
    /// When the source can't answer, such as for a cluster it doesn't have.
    fn query_feature_extension(
        _source: &Source,
        _feature: &crate::tile::feature::Feature,
        _extension: &str,
        _field: &str,
        _args: Option<&feature::PropertyMap>,
    ) -> Result<FeatureExtensionValue, Error> {
        Ok(FeatureExtensionValue::default())
    }
}

/// What a source supplies the renderer: tiles of the kind `K`, which a data
/// origin loads, or one image at four coordinates.
pub struct Supply<K>(pub(crate) Supplied, PhantomData<fn() -> K>);

pub(crate) enum Supplied {
    Tiles(Box<dyn ErasedOrigin>),
    Image(ImageOf),
}

/// The image of a source, if it is one of the type's.
pub type ImageOf = fn(&Source) -> Option<SourceImage>;

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
/// [`RasterTiles`] and [`DemTiles`].
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

/// The tiles of a raster source. A load delivers a tile's encoded image,
/// which the engine decodes. A layer type that draws raster images draws
/// them, or the one image of an image source ([`Supply::image`]), as its
/// input.
pub enum RasterTiles {}

impl TileKind for RasterTiles {
    type Item = Data;
}

/// The tiles of a raster-dem source. A load delivers a tile's encoded image,
/// which the engine decodes with the elevation encoding its origin set
/// ([`OriginUpdate::set_encoding`]). A layer type that draws elevations
/// draws them as its input.
pub enum DemTiles {}

impl TileKind for DemTiles {
    type Item = Data;
}

pub(crate) mod kind {
    use mln_style::layer::SourceKind;

    use super::{DemTiles, GeometryTiles, RasterTiles, TileKind};
    use crate::tile_store::tile_mut::TileMut;

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
            tile.take_image(item, false);
        }
    }

    impl Deliver for DemTiles {
        const KIND: SourceKind = SourceKind::RasterDem;

        fn deliver(tile: &mut TileMut<'_>, item: Option<<Self as TileKind>::Item>) {
            tile.take_image(item, false);
        }
    }
}

/// An image at coordinates: what an image source shows.
#[derive(Clone, Debug)]
pub struct SourceImage {
    /// The image, `None` until it arrives or for a source without one. The
    /// renderer uploads it again when it's another allocation.
    pub image: Option<Arc<PremultipliedImage>>,
    /// The image's corners: top left, top right, bottom right, bottom left.
    pub coordinates: [LatLng; 4],
}

/// What a source's tiles cover, and how the views they draw choose them:
/// their size in pixels, the zoom levels that have tiles of their own,
/// beyond which they are drawn scaled up, the bounds outside which there
/// are none, and how a view picks and lays out its tiles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cover {
    /// The size of the tiles, in pixels.
    pub tile_size: u16,
    /// The zoom levels that have tiles of their own.
    pub zoom_range: Range<u8>,
    /// The bounds outside which there are no tiles.
    pub bounds: Option<LatLngBounds>,
    /// Whether the tiles that cover a view are of the zoom level nearest the
    /// view's, as for raster tiles, rather than the level below it. A
    /// render in tile mode then draws every tile of the cover, not only
    /// the first.
    pub round_zoom: bool,
    /// Whether a view past the highest zoom level that has tiles asks for
    /// overscaled tiles of its own zoom, rather than drawing that level's
    /// tiles larger.
    pub overscale: bool,
    /// Whether, in continuous mode, the map first loads tiles a few zoom
    /// levels below the view's, and keeps the globe's lowest level loaded,
    /// so that something shows while the view's tiles load.
    pub prefetch: bool,
    /// Whether the tiles' geometry lays out in the update that delivers it,
    /// on the renderer's thread, rather than on the tile workers.
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

    /// Takes in the source as the style has it now, before the cover's
    /// update, in each of the renderer's updates: the same version until the
    /// style changes it, so the origin tells a new version from the last it
    /// took in, and then clears or loads the tiles again as the source's
    /// rules say. Returns what the tiles cover, or `None` while the source
    /// has nothing to make tiles from, in which case the engine leaves its
    /// tiles as they are and makes no new ones.
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

    /// The engine retries the required tile's failed loads, before each
    /// still image: it calls this for each required tile of the cover,
    /// failed or not, and the origin retries what of it failed. A tile that
    /// this begins a load of, or gives new data, loads anew; one that it
    /// leaves as it was stays as it was.
    fn retry(&mut self, _tile: &mut Self::Tile, _cx: &mut LoadContext<'_, '_, Self>) {}

    /// The engine couldn't lay out or decode what the tile last took in, and
    /// reported the error; the tile keeps what it shows. Not called for the
    /// origin's own [`LoadContext::fail`] or
    /// [`fail_retrying`](LoadContext::fail_retrying).
    fn failed(&mut self, _tile: &mut Self::Tile, _cx: &mut LoadContext<'_, '_, Self>) {}
}

/// What the map says of its tiles, which an origin reads as it updates
/// ([`OriginUpdate::map`]).
#[derive(Clone)]
pub struct MapSettings {
    /// Whether the map draws continuously or still images, which a tile
    /// shows only once it has done what it can.
    pub mode: MapMode,
    /// The map's pixel ratio, which a raster tile's resource may ask for.
    pub pixel_ratio: f32,
    /// The map's highest zoom level.
    pub max_zoom: f64,
    /// Where the map's resources load from, if anywhere.
    pub file_source: Option<Arc<dyn FileSource>>,
    /// Whether MLT tiles decode FastPFOR-compressed streams: an origin that
    /// delivers [`TileData::Mlt`] passes it to its
    /// [`MltTile`](crate::tile::MltTile), whose streams otherwise fail to
    /// decode as the tile lays out.
    pub fast_pfor_enabled: bool,
}

impl std::fmt::Debug for MapSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapSettings")
            .field("mode", &self.mode)
            .field("pixel_ratio", &self.pixel_ratio)
            .field("max_zoom", &self.max_zoom)
            .field("file_source", &self.file_source.as_ref().map(|_| format_args!("..")))
            .field("fast_pfor_enabled", &self.fast_pfor_enabled)
            .finish()
    }
}

/// A [`DataOrigin`]'s update: the source as the style has it, what the map
/// says of its tiles, and the tiles, which the origin can clear or load
/// again.
pub struct OriginUpdate<'a, O: ?Sized> {
    pub(crate) parts: UpdateParts<'a>,
    pub(crate) origin: PhantomData<fn(&mut O)>,
}

impl<'a, O: ?Sized> OriginUpdate<'a, O> {
    /// This update as one of the origin `P`'s, for an origin that runs its
    /// hooks through a non-generic core, such as one type that several
    /// generic origins share: what the update does to the tiles doesn't
    /// depend on the origin's type.
    pub fn cast<P: ?Sized>(&mut self) -> OriginUpdate<'_, P> {
        let parts = &mut self.parts;
        OriginUpdate {
            parts: UpdateParts {
                source: parts.source,
                tiles: &mut *parts.tiles,
                ctx: &mut *parts.ctx,
                map: parts.map,
                needs_relayout: parts.needs_relayout,
            },
            origin: PhantomData,
        }
    }

    /// The source as the style has it now.
    #[must_use]
    pub fn source(&self) -> &'a Source {
        self.parts.source
    }

    /// What the map says of its tiles.
    #[must_use]
    pub fn map(&self) -> &MapSettings {
        self.parts.map
    }

    /// The renderer's thread pool.
    #[must_use]
    pub fn pool(&self) -> &Pool {
        self.parts.ctx.pool.pool()
    }

    /// Whether the source's tiles lay out again.
    #[must_use]
    pub fn needs_relayout(&self) -> bool {
        self.parts.needs_relayout
    }

    /// Lets go of every tile. The origin's state of each drops, with no
    /// [`set_necessity`](DataOrigin::set_necessity) before.
    pub fn clear_tiles(&mut self) {
        self.parts.tiles.clear_all(self.parts.ctx);
    }

    /// Lets go of the cached tiles, those the view no longer needs. The
    /// origin's state of each drops, with no
    /// [`set_necessity`](DataOrigin::set_necessity) before, even for a tile
    /// that was still loading.
    pub fn clear_cache(&mut self) {
        self.parts.tiles.reduce_memory_use(self.parts.ctx);
    }
}

impl<O: DataOrigin> OriginUpdate<'_, O> {
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

impl<O: DataOrigin<Data = DemTiles>> OriginUpdate<'_, O> {
    /// Sets how the source's pixels encode elevations: the tiles made from
    /// now on decode with it, and the terrain's surface reads them with it.
    /// Mapbox's until an origin sets another.
    pub fn set_encoding(&mut self, encoding: RasterEncoding) {
        self.parts.tiles.info.dem_encoding = encoding;
    }
}

/// A tile, as a [`DataOrigin`]'s hook sees it: what the engine knows of it,
/// and what the origin can do to it.
pub struct LoadContext<'a, 't, O: ?Sized> {
    tile: &'a mut TileMut<'t>,
    origin: PhantomData<fn(&O)>,
}

impl<'a, 't, O: ?Sized> LoadContext<'a, 't, O> {
    pub(crate) fn new(tile: &'a mut TileMut<'t>) -> Self {
        LoadContext { tile, origin: PhantomData }
    }

    /// This context as one of the origin `P`'s, for an origin that runs its
    /// hooks through a non-generic core, as [`OriginUpdate::cast`]. The core
    /// begins loads and delivers through
    /// [`begin_load_of`](Self::begin_load_of) and
    /// [`deliver_of`](Self::deliver_of), which name the reply and the kind
    /// of data that the origin's own [`begin_load`](Self::begin_load) and
    /// [`deliver`](Self::deliver) would.
    pub fn cast<P: ?Sized>(&mut self) -> LoadContext<'_, 't, P> {
        LoadContext { tile: &mut *self.tile, origin: PhantomData }
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

    /// Sets when the tile's data was modified, when it expires, and until
    /// when it may be shown, which the engine's cache keeps to.
    pub fn set_times(&mut self, times: TileTimes) {
        self.tile.tile_mut().load.times = times;
    }

    /// Whether the tile can draw, if only some of its layers.
    #[must_use]
    pub fn is_renderable(&self) -> bool {
        self.tile.tile().is_renderable()
    }

    /// Whether the tile's geometry lays out on the renderer's thread as it
    /// takes it in, as a source that updates synchronously does.
    #[must_use]
    pub fn is_synchronous(&self) -> bool {
        self.tile.is_synchronous()
    }

    /// The renderer's thread pool.
    #[must_use]
    pub fn pool(&self) -> &Pool {
        self.tile.store().pool.pool()
    }

    /// [`begin_load`](Self::begin_load) of a load whose replies are `R`,
    /// which must be the [`Reply`](DataOrigin::Reply) of the tile's origin:
    /// the origin gets no other replies.
    pub fn begin_load_of<R>(&mut self) -> Replier<R> {
        let load = self.tile.next();
        let tile = self.tile.tile_mut();
        tile.load.current = Some(load);
        tile.load.failed = false;
        let tile = self.tile.key();
        Replier { inbox: self.tile.store().inbox.clone(), tile, load, reply: PhantomData }
    }

    /// [`deliver`](Self::deliver) for tiles of the kind `K`, which must be
    /// the [`Data`](DataOrigin::Data) of the tile's origin.
    pub fn deliver_of<K: TileKind>(&mut self, item: Option<K::Item>) {
        self.tile.tile_mut().load.failed = false;
        K::deliver(self.tile, item);
    }

    /// A load found that the source has no tile here: a tile of another
    /// zoom level draws in its place.
    pub fn missing(&mut self) {
        self.tile.tile_mut().load.failed = false;
        self.tile.take_missing();
    }

    /// The tile couldn't load: it keeps what it shows, and the engine
    /// reports `error`.
    pub fn fail(&mut self, error: Error) {
        self.tile.fail(error);
    }

    /// The tile's load failed, and waits to be tried again, as a network
    /// request that stays open while its file source backs off: as
    /// [`fail`](Self::fail), and until the origin begins a load or gives the
    /// tile anything ([`begin_load`](Self::begin_load),
    /// [`deliver`](Self::deliver), [`missing`](Self::missing),
    /// [`keep_data`](Self::keep_data) or [`WorldCopy::take`]), the tile
    /// counts as failed rather than loading: out of the view, one with
    /// nothing to show goes rather than wait in the cache for the retry, and
    /// is made anew if the view comes back.
    pub fn fail_retrying(&mut self, error: Error) {
        self.tile.tile_mut().load.failed = true;
        self.tile.fail(error);
    }

    /// A load found the tile's data unchanged: the tile has done what it
    /// can, unless work on its data is under way, which settles it as it
    /// ends.
    pub fn keep_data(&mut self) {
        self.tile.tile_mut().load.failed = false;
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

impl<O: DataOrigin + ?Sized> LoadContext<'_, '_, O> {
    /// Starts a load, the tile's latest from now on, whose replies the
    /// returned replier sends. Replies to the loads before go nowhere. A
    /// load stays the latest until the next one begins, so an origin that
    /// stops a load's work ignores the replies it sent before it stopped.
    pub fn begin_load(&mut self) -> Replier<O::Reply> {
        self.begin_load_of()
    }

    /// Gives the tile what a load brought, which it lays out or decodes:
    /// `None` for an empty tile, which shows nothing.
    pub fn deliver(&mut self, item: Option<<O::Data as TileKind>::Item>) {
        self.deliver_of::<O::Data>(item);
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
    /// and a raster or raster-dem tile keeps what it shows. Returns false,
    /// and changes nothing, if the copy has no data to give, as one whose
    /// decode is still pending.
    pub fn take<O: DataOrigin<Tile = T> + ?Sized>(self, cx: &mut LoadContext<'_, '_, O>) -> bool {
        self.take_into(cx)
    }

    /// [`take`](Self::take) through a context of any origin's, as a
    /// non-generic core takes it ([`LoadContext::cast`]).
    pub fn take_into<O: ?Sized>(self, cx: &mut LoadContext<'_, '_, O>) -> bool {
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
