//! The tests' engine half of the vector, raster and raster-dem source types:
//! the TileJSON at the source's URL or the tileset the style lists, whose
//! tiles each make one network request through the map's file source while
//! they are required. The request stays open, so the file source can
//! answer it again as it retries an error or revalidates data that
//! expired. Unlike the built-in type's, its tiles look in no cache first,
//! share no data with the tiles past the tileset's last zoom, which draw the
//! last zoom's scaled up, and decode no MLT.

use std::marker::PhantomData;
use std::sync::Arc;

use mln_style::error::Error;
use mln_style::source::SourceData;
use mln_style::sources::tileset::{
    RasterDemKind, RasterKind, TilesetKind, TilesetSourceOptions, TilesetSourceType, VectorKind,
};
use mln_style::tileset::{RasterEncoding, Scheme, Tileset};
use mln_style::{Source, constants};

use crate::source::{
    Cover, DataOrigin, DemTiles, Described, Fetch, GeometryTiles, Load, LoadContext, OfflineResources, OriginUpdate,
    RasterTiles, Respond, SourceType, Supply, TileKind, TileNecessity, TileOperation, TileTimes, WorldCopy,
};
use crate::storage::resource::TileFormat;
use crate::storage::response::{Caching, Reason};
use crate::storage::{Data, FileSource, LoadingMethod, Request, Resource, Response};
use crate::tile::{MvtTile, TileData};

/// The tests' engine half of the tileset source types of kind `K`.
pub(crate) struct TestTileset<K>(PhantomData<fn() -> K>);

/// A tileset kind's kind of tiles, and how a view covers them.
pub(crate) trait TestTilesetKind: TilesetKind + Sized {
    type Data: TileKind;
    const VECTOR: bool;

    /// Sets how the tiles' pixels encode elevations, for a raster-dem
    /// tileset.
    fn set_encoding(_update: &mut OriginUpdate<'_, TestTilesetOrigin<Self>>, _encoding: RasterEncoding) {}
}

impl TestTilesetKind for VectorKind {
    type Data = GeometryTiles;
    const VECTOR: bool = true;
}

impl TestTilesetKind for RasterKind {
    type Data = RasterTiles;
    const VECTOR: bool = false;
}

impl TestTilesetKind for RasterDemKind {
    type Data = DemTiles;
    const VECTOR: bool = false;

    fn set_encoding(update: &mut OriginUpdate<'_, TestTilesetOrigin<Self>>, encoding: RasterEncoding) {
        update.set_encoding(encoding);
    }
}

impl<K: TestTilesetKind> SourceType for TestTileset<K> {
    type Style = TilesetSourceType<K>;
    type Data = K::Data;
    type Description = Tileset;

    fn load(source: &SourceData<Self::Style>) -> Load<Tileset> {
        match &source.options.url {
            Some(url) => Load::Fetch(Fetch::Source(url.clone())),
            None => Load::Ready(source.options.tileset.clone().into()),
        }
    }

    fn describe(options: &TilesetSourceOptions, data: Option<Data>, respond: Respond<Described<Tileset>>) {
        let tileset = match data {
            None => Err("unexpectedly empty TileJSON".to_owned()),
            Some(data) => options.tileset_with_tilejson(&data, K::INFO.layer_kind),
        };
        respond(tileset.map(Some).map_err(Error::native));
    }

    fn metadata_changed(old: Option<&Tileset>, new: Option<&Tileset>) -> bool {
        old != new
    }

    fn attribution(source: &SourceData<Self::Style>) -> Option<String> {
        source.description::<Tileset>().map(|tileset| tileset.attribution.clone())
    }

    fn offline(source: &SourceData<Self::Style>) -> OfflineResources {
        let options = &source.options;
        let (tile_size, round_zoom) = (options.tile_size, !K::VECTOR);
        match &options.url {
            Some(url) => OfflineResources::TileJson { url: url.clone(), tile_size, round_zoom },
            None => match options.inline_tileset() {
                Some(tileset) => OfflineResources::Tiles { tileset, tile_size, round_zoom },
                None => OfflineResources::None,
            },
        }
    }

    fn offline_tileset(source: &SourceData<Self::Style>, tilejson: &[u8]) -> Result<Tileset, String> {
        source.options.tileset_with_tilejson(tilejson, K::INFO.layer_kind)
    }

    fn supply(_source: &Source) -> Supply<K::Data> {
        Supply::tiles(TestTilesetOrigin::<K> { tileset: None, pixel_ratio: 1.0, file_source: None, kind: PhantomData })
    }
}

/// A test tileset source's tiles: the tileset of the source's last version.
pub(crate) struct TestTilesetOrigin<K> {
    /// The tileset, while it has tiles.
    tileset: Option<Tileset>,
    /// The map's pixel ratio, which the tiles' resources ask for.
    pixel_ratio: f32,
    /// What the tiles load through.
    file_source: Option<Arc<dyn FileSource>>,
    kind: PhantomData<fn() -> K>,
}

/// A test tileset tile's request, while it makes one.
pub(crate) struct TestTilesetTile {
    pub(crate) resource: Resource,
    /// The open request; dropping it cancels it.
    request: Option<Request>,
    /// The request's last answer was an error.
    failed: bool,
    /// The tile has data, which never expires if `caching` says so.
    has_data: bool,
    caching: Caching,
}

impl TestTilesetTile {
    /// Whether a network request is open.
    pub(crate) fn is_requesting(&self) -> bool {
        self.request.is_some()
    }
}

impl<K: TestTilesetKind> TestTilesetOrigin<K> {
    /// Requests the tile's data, or fails it without a file source.
    fn request(&self, tile: &mut TestTilesetTile, cx: &mut LoadContext<'_, '_, Self>) {
        let Some(file_source) = &self.file_source else {
            cx.fail(Error::native("Can't load tile."));
            return;
        };
        cx.report(TileOperation::RequestedFromNetwork);
        let replier = cx.begin_load_of::<Response>();
        let (request, responder) = Request::new(move |response| replier.send(response));
        file_source.load(&tile.resource, responder);
        tile.failed = false;
        tile.request = Some(request);
    }

    /// Delivers what `res` brought: a 404 is a missing raster tile and an
    /// empty vector tile, a sparse tileset's empty tiles are missing.
    fn deliver(&self, res: &Response, cx: &mut LoadContext<'_, '_, Self>) {
        let sparse = self.tileset.as_ref().is_some_and(|tileset| tileset.sparse);
        let not_found = res.tile_not_found || res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        let empty = res.no_content || not_found;
        if empty && (sparse || (not_found && !K::VECTOR)) {
            cx.missing();
            return;
        }
        let data = if empty { None } else { res.data.clone() };
        if K::VECTOR {
            cx.deliver_of::<GeometryTiles>(data.map(|data| TileData::Mvt(MvtTile::new(data))));
        } else {
            cx.deliver_of::<RasterTiles>(data);
        }
    }
}

impl<K: TestTilesetKind> DataOrigin for TestTilesetOrigin<K> {
    type Data = K::Data;
    type Tile = TestTilesetTile;
    type Reply = Response;

    /// Takes in the source's tileset once it has loaded, clearing the
    /// tiles of another; on a static map, also before it has.
    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let source = SourceData::<TilesetSourceType<K>>::recover(update.source())?;
        let loaded = source.description::<Tileset>();
        self.pixel_ratio = update.map().pixel_ratio;
        self.file_source.clone_from(&update.map().file_source);
        let can_update = loaded.is_some() || update.map().mode != crate::map::MapMode::Continuous;
        if can_update && self.tileset.as_ref() != loaded {
            self.tileset = loaded.cloned();
            update.clear_tiles();
        }
        let tileset = self.tileset.as_ref().filter(|tileset| !tileset.tiles.is_empty())?;
        let cover = Cover {
            tile_size: if K::VECTOR { constants::TILE_SIZE_I } else { source.options.tile_size },
            zoom_range: tileset.zoom_range,
            bounds: tileset.bounds,
            round_zoom: !K::VECTOR,
            overscale: K::VECTOR,
            prefetch: true,
            synchronous: false,
        };
        let encoding = tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox);
        K::set_encoding(update, encoding);
        Some(cover)
    }

    /// A tile starts idle, unless it takes its copy's contents.
    fn start(
        &mut self,
        cx: &mut LoadContext<'_, '_, Self>,
        copy: Option<WorldCopy<'_, TestTilesetTile>>,
    ) -> TestTilesetTile {
        let (url, scheme) = self.tileset.as_ref().map_or(("", Scheme::default()), |tileset| {
            (tileset.tiles.first().map_or("", String::as_str), tileset.scheme)
        });
        let canonical = cx.id().canonical;
        let format = if K::VECTOR { TileFormat::Mvt } else { TileFormat::Image };
        let (x, y, z) = (canonical.x as i32, canonical.y as i32, canonical.z as i8);
        let mut resource = Resource::tile(url, self.pixel_ratio, x, y, z, scheme, format, LoadingMethod::NetworkOnly);
        resource.order = cx.order();
        let mut tile =
            TestTilesetTile { resource, request: None, failed: false, has_data: false, caching: Caching::default() };
        let taken = copy.and_then(|copy| {
            let state = copy.state();
            copy.take_into(cx).then_some(state)
        });
        match taken {
            Some(copy) => {
                (tile.caching, tile.has_data, tile.resource.prior_caching) =
                    (copy.caching, true, copy.resource.prior_caching);
            }
            None if self.tileset.is_some() && self.file_source.is_none() => cx.fail(Error::native("Can't load tile.")),
            None => {}
        }
        tile
    }

    /// Takes in an answer to the tile's open request. A streak of errors
    /// fails the tile once; the request stays open for the file source's
    /// retry.
    fn receive(&mut self, tile: &mut TestTilesetTile, res: Response, cx: &mut LoadContext<'_, '_, Self>) {
        if tile.request.is_none() {
            return;
        }
        if let Some(error) = res.error.as_ref().filter(|error| error.reason != Reason::NotFound) {
            if !std::mem::replace(&mut tile.failed, true) {
                tile.has_data = false;
                cx.fail_retrying(Error::native(error.message.clone()));
            }
            return;
        }
        tile.failed = false;
        if res.not_modified {
            tile.resource.prior_caching = res.caching;
        } else {
            tile.resource.set_prior(&res);
        }
        (tile.caching, tile.has_data) = (res.caching, true);
        cx.set_times(TileTimes {
            modified: res.modified,
            expires: res.caching.expires,
            usable_until: res.caching.usable_until(),
        });
        cx.report(TileOperation::LoadFromNetwork);
        if res.not_modified {
            cx.keep_data();
        } else {
            self.deliver(&res, cx);
        }
    }

    /// A required tile requests its data, unless it has data that never
    /// expires; an optional one stops its request.
    fn set_necessity(
        &mut self,
        tile: &mut TestTilesetTile,
        previous: TileNecessity,
        cx: &mut LoadContext<'_, '_, Self>,
    ) {
        if previous == cx.necessity() {
            return;
        }
        if cx.necessity() == TileNecessity::Required {
            let lasting = tile.has_data && tile.caching.expires.is_none();
            if tile.request.is_none() && !lasting {
                self.request(tile, cx);
            }
        } else {
            tile.request = None;
        }
    }

    /// Requests a tile whose request's last answer was an error again.
    fn retry(&mut self, tile: &mut TestTilesetTile, cx: &mut LoadContext<'_, '_, Self>) {
        if tile.failed {
            tile.request = None;
            self.request(tile, cx);
        }
    }

    /// The tile's data failed to lay out or decode: the tile loads it again
    /// when required again.
    fn failed(&mut self, tile: &mut TestTilesetTile, _cx: &mut LoadContext<'_, '_, Self>) {
        tile.has_data = false;
    }
}
