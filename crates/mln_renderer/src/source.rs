//! The source type contract: what a crate implements to add a source type
//! that the renderer draws.
//!
//! A source type is a marker type that implements `mln_style`'s
//! `StyleSourceType`, its style half, and [`SourceType`], its render half. A
//! [`RegistryBuilder`](crate::RegistryBuilder) registers it by name with
//! [`add_source_type`](crate::RegistryBuilder::add_source_type).
//!
//! A source [supplies](SourceType::supply) the renderer one of the engine's
//! kinds of data: tiles of a [`TileKind`] (geometry, a raster image or a
//! raster-dem elevation model), or one image at four coordinates
//! ([`SourceImage`]). A new source of data is a source type; a new kind of
//! data is a change to the engine.
//!
//! A source's tiles get their data from its [`DataOrigin`]. The engine
//! decides which tiles the view needs, keeps them, and lays out or decodes
//! what they load. With each new version of the source, the origin updates
//! ([`DataOrigin::update`]) and says what its tiles cover ([`Cover`]). It
//! keeps a [`TileLoad`] of each tile, and owns the tile's loading through
//! hooks that the engine calls as the tile starts, as its
//! [necessity](TileNecessity) and [update parameters](TileUpdateParameters)
//! change, when it retries after a failure, and when a reply to one of its
//! loads arrives. Each hook gets the tile's [`LoadContext`]: through it the
//! origin starts a load, whose [`Replier`] sends its replies from any
//! thread, and gives the tile what the load brought as a [`TileInput`] of
//! the tile's kind. The engine hands a reply to the origin only while its
//! load is the tile's latest ([`LoadContext::begin_load`]).

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use mln_map_types::MapMode;
use mln_storage::{FileSource, RequestOrder};
use mln_style::Source;
use mln_style::source_type::StyleSourceType;
use mln_tile_id::OverscaledTileID;
use mln_util::LatLng;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::range::Range;
use mln_util::tileset::RasterEncoding;

use crate::erased_source::{ErasedOrigin, UpdateParts, typed};
pub use crate::query::FeatureExtensionValue;
use crate::tile::inbox::{InboxSender, TileEvent};
use crate::tile::job::TileKey;
pub use crate::tile::loading::Freshness;
pub use crate::tile::tile::{TileInput, TileKind, TileNecessity, TileOperation, TileUpdateParameters};
use crate::tile::tile_mut::TileMut;

/// The render half of a source type, whose style half is its
/// [`StyleSourceType`]: what its sources supply the renderer.
pub trait SourceType: StyleSourceType {
    /// What `source`, a source of the type, supplies the renderer, which
    /// makes it once, as the source joins the renderer's style. The data is
    /// of the kind the type's [`layer_kind`](mln_style::source_type::SourceTypeInfo::layer_kind)
    /// names: geometry tiles for `Geometry`, raster tiles or an image for
    /// `Raster`, raster-dem tiles for `RasterDem`.
    fn supply(source: &Source) -> Supply;
}

/// What a source supplies the renderer: tiles of one kind, which a data
/// origin loads, or one image at four coordinates.
pub struct Supply(pub(crate) Supplied);

pub(crate) enum Supplied {
    Tiles { kind: TileKind, single_layer: bool, origin: Box<dyn ErasedOrigin> },
    Image(ImageOf),
}

/// The image of a source, if it is one of the type's.
pub type ImageOf = for<'a> fn(&'a Source) -> Option<SourceImage<'a>>;

impl Supply {
    /// Tiles of `kind`, whose data comes from `origin`.
    #[must_use]
    pub fn tiles<O: DataOrigin>(kind: TileKind, origin: O) -> Self {
        Supply(Supplied::Tiles { kind, single_layer: false, origin: Box::new(origin) })
    }

    /// Tiles of `kind` that each hold one source layer, which a source
    /// query gets whatever source layer it names, whose data comes from
    /// `origin`.
    #[must_use]
    pub fn single_layer_tiles<O: DataOrigin>(kind: TileKind, origin: O) -> Self {
        Supply(Supplied::Tiles { kind, single_layer: true, origin: Box::new(origin) })
    }

    /// One image, which `image_of` reads from each version of the source.
    #[must_use]
    pub fn image(image_of: ImageOf) -> Self {
        Supply(Supplied::Image(image_of))
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

/// What a source's tiles cover: their size in pixels, the zoom levels that
/// have tiles of their own, beyond which they are drawn scaled up, and the
/// bounds outside which there are none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cover {
    pub tile_size: u16,
    pub zoom_range: Range<u8>,
    pub bounds: Option<LatLngBounds>,
}

/// Where a source's tiles get their data: what each tile has of the
/// source's data, and how it loads.
///
/// The tile hooks ([`start`](Self::start), [`receive`](Self::receive),
/// [`set_necessity`](Self::set_necessity),
/// [`update_parameters_changed`](Self::update_parameters_changed) and
/// [`retry`](Self::retry)) get the tile's [`Load`](Self::Load) and its
/// [`LoadContext`]. The engine calls none of the hooks from within another.
pub trait DataOrigin: 'static {
    /// What the origin keeps of each tile's loading.
    type Load: TileLoad;

    /// What a tile's load gets back, which [`receive`](Self::receive)
    /// takes in.
    type Reply: Send + 'static;

    /// Takes in a new version of the source, or its first: clears or loads
    /// the tiles again as the source's rules say. Returns what the tiles
    /// cover, or `None` while the source has nothing to make tiles from,
    /// in which case the engine leaves its tiles as they are.
    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover>;

    /// The load of a new tile, `tile`; `None` while the origin has nothing
    /// to make tiles from, which leaves the tile out.
    fn new_load(&mut self, tile: &NewTile<'_>) -> Option<Self::Load>;

    /// Starts the load of a new tile, required or not.
    fn start(&mut self, load: &mut Self::Load, cx: &mut LoadContext<'_, '_, Self::Reply>);

    /// Takes in `reply`, which came for the tile's latest load.
    fn receive(&mut self, load: &mut Self::Load, reply: Self::Reply, cx: &mut LoadContext<'_, '_, Self::Reply>);

    /// The engine set how much the tile is needed, to
    /// [`cx.necessity()`](LoadContext::necessity) from `previous`, which may
    /// be the same: as each update's cover retains the tile, and as it
    /// becomes optional. An optional tile only takes low-cost actions to
    /// get its data, such as looking in the cache.
    fn set_necessity(
        &mut self,
        _load: &mut Self::Load,
        _previous: TileNecessity,
        _cx: &mut LoadContext<'_, '_, Self::Reply>,
    ) {
    }

    /// The tile's [update parameters](LoadContext::update_parameters)
    /// changed.
    fn update_parameters_changed(&mut self, _load: &mut Self::Load, _cx: &mut LoadContext<'_, '_, Self::Reply>) {}

    /// The engine retries the required tile's failed loads.
    fn retry(&mut self, _load: &mut Self::Load, _cx: &mut LoadContext<'_, '_, Self::Reply>) {}

    /// Has `load`, of a new tile, take what `from` brought, the load of the
    /// same tile in another world copy, whose contents the new tile starts
    /// with: as a cache's answer, so that the tile settles without loading.
    /// Returns false if the tile loads anew.
    fn take_world_copy(&self, _load: &mut Self::Load, _from: &Self::Load) -> bool {
        false
    }

    /// The value of a feature extension of a feature of the source's
    /// tiles; a null value for an origin without extensions.
    ///
    /// # Errors
    ///
    /// When the extension fails to answer.
    fn query_feature_extensions(
        &self,
        _feature: &mln_util::feature::Feature,
        _extension: &str,
        _extension_field: &str,
        _args: Option<&feature::PropertyMap>,
    ) -> Result<FeatureExtensionValue, Error> {
        Ok(FeatureExtensionValue::default())
    }
}

/// What a [`DataOrigin`] keeps of a tile's loading.
pub trait TileLoad: Any {
    /// The tile `tile` goes: lets go of what the load holds outside the
    /// tile, such as its place among a provider's subscribers. The engine
    /// drops the load after. Not called as the renderer itself drops; what
    /// must go then goes in `Drop`.
    fn retire(&mut self, _tile: &OverscaledTileID) {}
}

/// A new tile, whose load a [`DataOrigin`] makes.
pub struct NewTile<'a> {
    pub(crate) id: &'a OverscaledTileID,
    pub(crate) pixel_ratio: f32,
    pub(crate) order: &'a RequestOrder,
}

impl NewTile<'_> {
    #[must_use]
    pub fn id(&self) -> &OverscaledTileID {
        self.id
    }

    /// The pixel ratio of the map, which a raster tile's resource asks for.
    #[must_use]
    pub fn pixel_ratio(&self) -> f32 {
        self.pixel_ratio
    }

    /// When the tile's requests start among those that wait for a
    /// connection, which the engine keeps up to date: for the tile's
    /// resources.
    #[must_use]
    pub fn order(&self) -> RequestOrder {
        self.order.clone()
    }
}

/// A [`DataOrigin`]'s update: the source's new and previous versions, what
/// the map says of its tiles, and the tiles, which the origin can clear or
/// load again.
pub struct OriginUpdate<'a, O: ?Sized> {
    pub(crate) parts: UpdateParts<'a>,
    pub(crate) origin: PhantomData<fn(&mut O)>,
}

impl<'a, O: DataOrigin> OriginUpdate<'a, O> {
    /// The source's version before this one; the same as
    /// [`source`](Self::source) at its first update.
    #[must_use]
    pub fn previous(&self) -> &'a Source {
        self.parts.previous
    }

    #[must_use]
    pub fn source(&self) -> &'a Source {
        self.parts.source
    }

    #[must_use]
    pub fn mode(&self) -> MapMode {
        self.parts.parameters.mode
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

    /// Whether the source's tiles lay out in the update that asks for them.
    #[must_use]
    pub fn is_synchronous(&self) -> bool {
        self.parts.parameters.is_update_synchronous
    }

    /// Whether the source's tiles lay out again.
    #[must_use]
    pub fn needs_relayout(&self) -> bool {
        self.parts.needs_relayout
    }

    /// Whether MLT tiles decode FastPFOR-compressed streams.
    #[must_use]
    pub fn fast_pfor_enabled(&self) -> bool {
        self.parts.fast_pfor_enabled
    }

    /// Lets go of every tile.
    pub fn clear_tiles(&mut self) {
        self.parts.tiles.clear_all(self.parts.ctx);
    }

    /// Lets go of the cached tiles beyond what the cache keeps with less
    /// memory.
    pub fn reduce_memory_use(&mut self) {
        self.parts.tiles.reduce_memory_use(self.parts.ctx);
    }

    /// Decodes the elevations of the tiles made from now on, of a
    /// raster-dem source, as `encoding` says.
    pub fn set_dem_encoding(&mut self, encoding: RasterEncoding) {
        if let TileKind::Dem(current) = &mut self.parts.tiles.info.kind {
            *current = encoding;
        }
    }

    /// Changes the load of each tile of the cover, not of the cache, that
    /// `filter` picks, through `f`.
    pub fn update_tiles(
        &mut self,
        filter: impl Fn(&OverscaledTileID) -> bool,
        mut f: impl FnMut(&mut O::Load, &mut LoadContext<'_, '_, O::Reply>),
    ) {
        self.parts.tiles.update_each(
            self.parts.ctx,
            |slot| !slot.place().is_cached() && filter(&slot.tile.id),
            |tile| {
                tile.with_load(|load, tile| {
                    if let Some(load) = typed::<O>(load) {
                        f(load, &mut LoadContext::new(tile));
                    }
                });
            },
        );
    }
}

/// A tile, as a [`DataOrigin`]'s hook sees it: what the engine knows of it,
/// and what the origin can do to it.
pub struct LoadContext<'a, 't, R> {
    tile: &'a mut TileMut<'t>,
    reply: PhantomData<fn(R)>,
}

impl<'a, 't, R: Send + 'static> LoadContext<'a, 't, R> {
    pub(crate) fn new(tile: &'a mut TileMut<'t>) -> Self {
        LoadContext { tile, reply: PhantomData }
    }

    #[must_use]
    pub fn id(&self) -> &OverscaledTileID {
        &self.tile.tile().id
    }

    /// The kind of the tile, which the input it takes is of.
    #[must_use]
    pub fn kind(&self) -> TileKind {
        self.tile.source().kind
    }

    #[must_use]
    pub fn necessity(&self) -> TileNecessity {
        self.tile.tile().load.necessity
    }

    /// How the tile's network loads keep its data fresh.
    #[must_use]
    pub fn update_parameters(&self) -> TileUpdateParameters {
        self.tile.tile().load.update_parameters
    }

    /// When the tile's data was modified, when it expires, and whether it
    /// came from the network.
    #[must_use]
    pub fn freshness(&self) -> Freshness {
        self.tile.tile().load.freshness
    }

    pub fn set_freshness(&mut self, freshness: Freshness) {
        self.tile.tile_mut().load.freshness = freshness;
    }

    /// Whether the tile can draw, if only some of its layers.
    #[must_use]
    pub fn is_renderable(&self) -> bool {
        self.tile.tile().is_renderable()
    }

    /// Whether the tile's work runs in the update that asks for it, on
    /// this thread.
    #[must_use]
    pub fn is_synchronous(&self) -> bool {
        self.tile.source().synchronous
    }

    /// The renderer's thread pool.
    #[must_use]
    pub fn pool(&self) -> &Pool {
        self.tile.store().pool.pool()
    }

    /// Starts a load, the tile's latest from now on, whose replies the
    /// returned replier sends. Replies to the loads before go nowhere.
    pub fn begin_load(&mut self) -> Replier<R> {
        let load = self.tile.next();
        self.tile.tile_mut().load.current = Some(load);
        let tile = self.tile.key();
        Replier { inbox: self.tile.store().inbox.clone(), tile, load, reply: PhantomData }
    }

    /// Ends the tile's latest load: no replies go to the tile until it
    /// begins another.
    pub fn end_load(&mut self) {
        self.tile.tile_mut().load.current = None;
    }

    /// Gives the tile what a load brought, which it lays out or decodes.
    /// `input` is of the tile's [`kind`](Self::kind).
    pub fn deliver(&mut self, input: TileInput) {
        self.tile.take_input(input);
    }

    /// The tile couldn't load: it keeps what it shows, and its data isn't
    /// fresh.
    pub fn fail(&mut self, error: Error) {
        self.tile.fail(error);
    }

    /// A load found the tile's data unchanged: the tile has done what it
    /// can, unless work on its data is under way, which settles it as it
    /// ends.
    pub fn keep_data(&mut self) {
        self.tile.keep_data();
    }

    /// The tile loads again after a failure: it hasn't done what it can.
    pub fn unsettle(&mut self) {
        self.tile.unsettle();
    }

    /// Drops the tile's data and style, so that it lays out anew once it
    /// has both again.
    pub fn reset(&mut self) {
        self.tile.reset();
    }

    /// The cache answered the tile: an optional tile has done what it can,
    /// and a parent of a tile that needs it stands in for it.
    pub fn cache_answered(&mut self) {
        self.tile.tile_mut().load.cache_answered = true;
    }

    /// Reports what the tile's load did: requested from the cache or the
    /// network, or loaded from either.
    pub fn report(&mut self, op: TileOperation) {
        self.tile.action(op);
    }

    /// The tile changed: the renderer needs a frame.
    pub fn changed(&mut self) {
        self.tile.store_mut().changed();
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
    /// Which load the replier sends for. No two of a source's loads share
    /// it.
    #[must_use]
    pub fn load(&self) -> u64 {
        self.load
    }

    /// Sends `reply`, which the tile takes in at the renderer's next update
    /// if the load is still its latest.
    pub fn send(&self, reply: R) {
        self.inbox.send(TileEvent::Load { tile: self.tile, load: self.load, reply: Box::new(reply) });
    }
}
