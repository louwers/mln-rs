//! Source types for the tests of this crate, which can't reach the built-in
//! source types outside it: `tiles`, `raster-tiles` and `dem-tiles`, whose
//! geometry, raster and raster-dem tiles are those of a tileset that the
//! source holds, loaded through the map's file source.
//!
//! A tile loads as a tileset source's tile does from a file source without
//! a cache: a required tile requests its data from the network, unless it
//! has fresh data from there, an optional one stops the request, and a
//! failed one asks again when retried. A response without content is an
//! empty tile, and one that finds no tile a missing one, but for a geometry
//! tile, which is empty. A tile that starts as another world copy's takes
//! that copy's validators.

use std::marker::PhantomData;
use std::sync::Arc;

use mln_conversion::{Convertible, Error as ConversionError};
use mln_storage::response::Reason;
use mln_storage::{FileSource, LoadingMethod, Request, Resource, Response, StoragePolicy};
use mln_style::Source;
use mln_style::expression::Functions;
use mln_style::layer_type::SourceKind;
use mln_style::source_type::{SourceData, SourceTypeInfo, StyleSourceType};
use mln_tile::TileData;
use mln_util::error::Error;
use mln_util::tileset::{RasterEncoding, Tileset};

use crate::source::{
    Cover, DataOrigin, Freshness, LoadContext, NewTile, OriginUpdate, SourceType, Supply, TileInput, TileKind,
    TileLoad, TileNecessity, TileOperation,
};

/// The kind of a fixture source's tiles.
pub(crate) trait Kind: Send + Sync + 'static {
    const INFO: SourceTypeInfo;
    const TILES: TileKind;
}

pub(crate) struct Geometry;
pub(crate) struct Raster;
pub(crate) struct Dem;

/// The members of a fixture source type's [`SourceTypeInfo`], those of the
/// tileset source type of the same kind of tiles.
const fn info(name: &'static str, layer_kind: SourceKind, round_zoom: bool, overscale: bool) -> SourceTypeInfo {
    SourceTypeInfo { name, layer_kind, from_json: false, round_zoom, overscale, prefetch: true }
}

impl Kind for Geometry {
    const INFO: SourceTypeInfo = info("tiles", SourceKind::Geometry, false, true);
    const TILES: TileKind = TileKind::Geometry;
}

impl Kind for Raster {
    const INFO: SourceTypeInfo = info("raster-tiles", SourceKind::Raster, true, false);
    const TILES: TileKind = TileKind::Raster;
}

impl Kind for Dem {
    const INFO: SourceTypeInfo = info("dem-tiles", SourceKind::RasterDem, true, true);
    // The tileset decides the encoding.
    const TILES: TileKind = TileKind::Dem(RasterEncoding::Mapbox);
}

/// A fixture source type of tiles of the kind `K`.
pub(crate) struct Tiles<K>(PhantomData<fn() -> K>);

/// A fixture source's options: its tileset, of tiles `tile_size` pixels
/// wide.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TilesOptions {
    pub(crate) tileset: Tileset,
    pub(crate) tile_size: u16,
}

impl<K: Kind> StyleSourceType for Tiles<K> {
    const INFO: SourceTypeInfo = K::INFO;

    type Options = TilesOptions;
    type Description = ();

    fn from_json(
        _value: &Convertible<'_>,
        _functions: &Arc<Functions>,
        error: &mut ConversionError,
    ) -> Option<SourceData<Self>> {
        error.message = "a fixture source has no style JSON".to_owned();
        None
    }
}

impl<K: Kind> SourceType for Tiles<K> {
    fn supply(_source: &Source) -> Supply {
        Supply::tiles(K::TILES, Fetched::<K> { tileset: None, file_source: None, kind: PhantomData })
    }
}

/// The fixture source `id` of tiles of the kind `K`, `tile_size` pixels
/// wide, from `tileset`.
pub(crate) fn source<K: Kind>(id: &str, tileset: Tileset, tile_size: u16) -> Source {
    Source::new(id, SourceData::<Tiles<K>>::new(TilesOptions { tileset, tile_size }))
}

/// A fixture source's tiles: the tileset of its last version, and the file
/// source its tiles load through.
pub(crate) struct Fetched<K> {
    tileset: Option<Tileset>,
    file_source: Option<Arc<dyn FileSource>>,
    kind: PhantomData<fn() -> K>,
}

/// A fixture tile's request: its resource, which keeps the validators of
/// the last response, and the request under way.
pub(crate) struct FetchedLoad {
    resource: Resource,
    pub(crate) fetch: Fetch,
}

impl TileLoad for FetchedLoad {}

/// A fixture tile's request; dropping it cancels it.
pub(crate) enum Fetch {
    Idle,
    /// The last response was an error: `retry` asks again.
    Failed,
    Network {
        #[expect(dead_code, reason = "dropping the request cancels it")]
        request: Request,
    },
}

impl FetchedLoad {
    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.fetch, Fetch::Idle | Fetch::Failed)
    }

    /// Drops the request under way, whose response no longer applies.
    fn stop(&mut self, cx: &mut LoadContext<'_, '_, Response>) {
        self.fetch = Fetch::Idle;
        cx.end_load();
    }
}

impl<K: Kind> Fetched<K> {
    /// An origin whose tiles load from `tileset` through `file_source`, as
    /// if a source of it had updated.
    pub(crate) fn loaded(tileset: Tileset, file_source: Arc<dyn FileSource>) -> Self {
        Fetched { tileset: Some(tileset), file_source: Some(file_source), kind: PhantomData }
    }

    /// Requests the tile's data from the network.
    fn request(&self, load: &mut FetchedLoad, cx: &mut LoadContext<'_, '_, Response>) {
        let Some(file_source) = &self.file_source else {
            cx.fail(Error::native("Can't load tile."));
            return;
        };
        cx.report(TileOperation::RequestedFromNetwork);
        let replier = cx.begin_load();
        let parameters = cx.update_parameters();
        let resource = &mut load.resource;
        resource.minimum_update_interval = parameters.minimum_update_interval;
        resource.storage_policy =
            if parameters.is_volatile { StoragePolicy::Volatile } else { StoragePolicy::Permanent };
        let (request, responder) = Request::once(move |response| replier.send(response));
        file_source.load(resource, responder);
        load.fetch = Fetch::Network { request };
    }
}

impl<K: Kind> DataOrigin for Fetched<K> {
    type Load = FetchedLoad;
    type Reply = Response;

    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let data = SourceData::<Tiles<K>>::recover(update.source())?;
        let tileset = &data.options.tileset;
        if self.tileset.as_ref() != Some(tileset) {
            update.clear_tiles();
            self.tileset = Some(tileset.clone());
        }
        self.file_source = update.file_source().cloned();
        if tileset.tiles.is_empty() {
            return None;
        }
        update.set_dem_encoding(tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox));
        Some(Cover { tile_size: data.options.tile_size, zoom_range: tileset.zoom_range, bounds: tileset.bounds })
    }

    fn new_load(&mut self, tile: &NewTile<'_>) -> Option<FetchedLoad> {
        let tileset = self.tileset.as_ref().filter(|tileset| !tileset.tiles.is_empty())?;
        let id = tile.id().canonical;
        let mut resource = Resource::tile(
            &tileset.tiles[0],
            tile.pixel_ratio(),
            id.x as i32,
            id.y as i32,
            id.z as i8,
            tileset.scheme,
            LoadingMethod::NetworkOnly,
        );
        resource.order = tile.order();
        Some(FetchedLoad { resource, fetch: Fetch::Idle })
    }

    /// A tile waits until it is required.
    fn start(&mut self, _load: &mut FetchedLoad, cx: &mut LoadContext<'_, '_, Response>) {
        if self.file_source.is_none() {
            cx.fail(Error::native("Can't load tile."));
        }
    }

    fn receive(&mut self, load: &mut FetchedLoad, res: Response, cx: &mut LoadContext<'_, '_, Response>) {
        if !matches!(load.fetch, Fetch::Network { .. }) {
            return;
        }
        load.stop(cx);
        let not_found = res.tile_not_found || res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        if let Some(error) = &res.error
            && !not_found
        {
            load.fetch = Fetch::Failed;
            cx.fail(Error::native(error.message.clone()));
            return;
        }
        let resource = &mut load.resource;
        if !res.not_modified {
            resource.prior_modified = res.modified;
            resource.prior_etag = res.etag.clone();
        }
        resource.prior_expires = res.expires;
        cx.set_freshness(Freshness { modified: res.modified, expires: res.expires, from_network: true });
        cx.report(TileOperation::LoadFromNetwork);
        if res.not_modified {
            cx.keep_data();
            return;
        }
        let empty = res.no_content || not_found;
        let data = if empty { None } else { res.data.clone() };
        cx.deliver(match cx.kind() {
            TileKind::Geometry => TileInput::Geometry {
                data: data.map(|data| TileData::Mvt(mln_tile::MvtTile::new(data))),
                missing: false,
            },
            TileKind::Raster | TileKind::Dem(_) => TileInput::Image { data, missing: not_found },
        });
    }

    fn set_necessity(
        &mut self,
        load: &mut FetchedLoad,
        previous: TileNecessity,
        cx: &mut LoadContext<'_, '_, Response>,
    ) {
        if previous == cx.necessity() {
            return;
        }
        if cx.necessity() == TileNecessity::Required {
            if load.is_idle() && !cx.freshness().is_fresh(mln_util::chrono::now()) {
                self.request(load, cx);
            }
        } else if matches!(load.fetch, Fetch::Network { .. }) {
            load.stop(cx);
        }
    }

    fn update_parameters_changed(&mut self, load: &mut FetchedLoad, cx: &mut LoadContext<'_, '_, Response>) {
        if matches!(load.fetch, Fetch::Network { .. }) {
            load.stop(cx);
            self.request(load, cx);
        }
    }

    fn retry(&mut self, load: &mut FetchedLoad, cx: &mut LoadContext<'_, '_, Response>) {
        if matches!(load.fetch, Fetch::Failed) {
            load.fetch = Fetch::Idle;
            cx.unsettle();
            self.request(load, cx);
        }
    }

    fn take_world_copy(&self, load: &mut FetchedLoad, from: &FetchedLoad) -> bool {
        let (resource, from) = (&mut load.resource, &from.resource);
        resource.prior_modified = from.prior_modified;
        resource.prior_expires = from.prior_expires;
        resource.prior_etag.clone_from(&from.prior_etag);
        true
    }
}
