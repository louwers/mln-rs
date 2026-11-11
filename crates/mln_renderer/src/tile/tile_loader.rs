// Ports src/mln/tile/tile_loader.hpp and src/mln/tile/tile_loader_impl.hpp
// (for VectorTile; raster tiles later).
//
// C++ `TileLoader<T>` holds `T& tile` and request callbacks capture `this`.
// Here the loader state is an `Rc<RefCell<..>>` the callbacks reach through
// a `Weak` (a dropped loader = C++ `shared->aborted`), and the tile is the
// GeometryTile's render-thread state.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_storage::response::Reason;
use mln_storage::{AsyncRequest, Data, FileSource, LoadingMethod, Resource, Response, StoragePolicy};
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::Duration;
use mln_util::exception::{ExceptionPtr, RuntimeError};
use mln_util::tileset::Tileset;

use crate::tile::geometry_tile::GeometryTileInner;
use crate::tile::geometry_tile_worker::SharedTileData;
use crate::tile::tile::{TileNecessity, TileOperation, TileUpdateParameters};
use crate::tile_parameters::TileParameters;

/// `getCantLoadTileError()`.
fn get_cant_load_tile_error() -> ExceptionPtr {
    RuntimeError::ptr("Can't load tile.")
}

/// The tile data encoding of a vector tile: VectorMVTTile or VectorMLTTile
/// (raster tiles ignore it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorTileEncoding {
    Mvt,
    /// `VectorMLTTile` with `fastPFOREnabled`.
    Mlt {
        fast_pfor_enabled: bool,
    },
}

/// What a loader does with the tile it loads (`T` in C++).
pub trait LoaderTile {
    fn set_error(&self, err: ExceptionPtr);
    fn set_tried_cache(&self);
    fn on_tile_action(&self, op: TileOperation);
    fn set_metadata(&self, modified: Option<mln_util::chrono::Timestamp>, expires: Option<mln_util::chrono::Timestamp>);
    fn set_data(&self, data: Option<Data>, encoding: VectorTileEncoding);
}

/// VectorTile's half of the loader interface (vector_tile.cpp,
/// vector_mvt_tile.cpp, vector_mlt_tile.cpp).
impl LoaderTile for GeometryTileInner {
    fn set_error(&self, err: ExceptionPtr) {
        GeometryTileInner::set_error(self, err);
    }

    fn set_tried_cache(&self) {
        self.base.set_tried_cache();
    }

    fn on_tile_action(&self, op: TileOperation) {
        self.base.on_tile_action(op);
    }

    /// `VectorTile::setMetadata(modified, expires)`.
    fn set_metadata(
        &self,
        modified: Option<mln_util::chrono::Timestamp>,
        expires: Option<mln_util::chrono::Timestamp>,
    ) {
        self.base.modified.set(modified);
        self.base.expires.set(expires);
    }

    /// `VectorMVTTile::setData(data)` / `VectorMLTTile::setData(data)`.
    fn set_data(&self, data: Option<Data>, encoding: VectorTileEncoding) {
        if !self.is_obsolete() {
            let data: Option<SharedTileData> = data.map(|data| match encoding {
                VectorTileEncoding::Mvt => Arc::new(mln_tile::VectorMVTTileData::new(data)) as SharedTileData,
                VectorTileEncoding::Mlt { fast_pfor_enabled } => {
                    Arc::new(mln_tile::VectorMLTTileData::new(data, fast_pfor_enabled)) as SharedTileData
                }
            });
            GeometryTileInner::set_data(self, data);
        }
    }
}

struct State {
    tile: Weak<dyn LoaderTile>,
    encoding: VectorTileEncoding,
    necessity: TileNecessity,
    resource: Resource,
    file_source: Option<Arc<dyn FileSource>>,
    request: Option<Box<dyn AsyncRequest>>,
    update_parameters: TileUpdateParameters,
}

/// `TileLoader<T>`.
pub struct TileLoader {
    state: Rc<RefCell<State>>,
}

fn tile_of(state: &Rc<RefCell<State>>) -> Option<Rc<dyn LoaderTile>> {
    state.borrow().tile.upgrade()
}

impl TileLoader {
    /// `TileLoader(T& tile, const OverscaledTileID&, const TileParameters&,
    /// const Tileset&)`.
    pub fn new(
        tile: Weak<dyn LoaderTile>,
        encoding: VectorTileEncoding,
        id: &OverscaledTileID,
        parameters: &TileParameters<'_>,
        tileset: &Tileset,
    ) -> Self {
        // The render sources make no tiles for a tileset without tiles; such
        // a tile can't load (C++ `tileset.tiles.at(0)` throws).
        let url = tileset.tiles.first().map_or("", String::as_str);
        let resource = Resource::tile(
            url,
            parameters.pixel_ratio,
            id.canonical.x as i32,
            id.canonical.y as i32,
            id.canonical.z as i8,
            tileset.scheme,
            LoadingMethod::CacheOnly,
        );
        let state = Rc::new(RefCell::new(State {
            tile,
            encoding,
            necessity: TileNecessity::Optional,
            resource,
            file_source: if tileset.tiles.is_empty() { None } else { parameters.file_source.clone() },
            request: None,
            update_parameters: TileUpdateParameters { minimum_update_interval: Duration::zero(), is_volatile: false },
        }));
        let loader = TileLoader { state };

        let file_source = loader.state.borrow().file_source.clone();
        let Some(file_source) = file_source else {
            if let Some(tile) = tile_of(&loader.state) {
                tile.set_error(get_cant_load_tile_error());
            }
            return loader;
        };

        if file_source.supports_cache_only_requests() {
            // When supported, the first request is always optional, even if
            // the TileLoader is marked as required. That way, we can let the
            // first optional request continue to load when the TileLoader is
            // later changed from required to optional. If we started out
            // with a required request, we'd have to cancel everything,
            // including the initial optional part of the request.
            load_from_cache(&loader.state);
        } else if loader.state.borrow().necessity == TileNecessity::Required {
            // When the file source doesn't support cache-only requests, and
            // we definitely need this data, we can start out with a network
            // request immediately.
            load_from_network(&loader.state);
        } else {
            // When the FileSource doesn't support cache-only requests, we do
            // nothing until the data is definitely required.
        }
        loader
    }

    pub fn set_necessity(&self, new_necessity: TileNecessity) {
        let changed = {
            let mut state = self.state.borrow_mut();
            if new_necessity != state.necessity {
                state.necessity = new_necessity;
                true
            } else {
                false
            }
        };
        if changed {
            if new_necessity == TileNecessity::Required {
                make_required(&self.state);
            } else {
                make_optional(&self.state);
            }
        }
    }

    pub fn set_update_parameters(&self, params: &TileUpdateParameters) {
        let reload = {
            let mut state = self.state.borrow_mut();
            if state.update_parameters != *params {
                state.update_parameters = *params;
                if has_pending_network_request(&state) {
                    // Update the pending request.
                    state.request = None;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if reload {
            load_from_network(&self.state);
        }
    }
}

fn has_pending_network_request(state: &State) -> bool {
    state.resource.loading_method == LoadingMethod::NetworkOnly && state.request.is_some()
}

/// `makeRequired()`: called when the tile is one of the ideal tiles that we
/// want to show definitely. The tile source should try to make every effort
/// (e.g. fetch from internet, or revalidate existing resources).
fn make_required(state: &Rc<RefCell<State>>) {
    if state.borrow().request.is_none() {
        load_from_network(state);
    }
}

/// `makeOptional()`: called when the zoom level no longer corresponds to the
/// displayed one, but we're still interested in retaining the tile, e.g. for
/// backfill.
fn make_optional(state: &Rc<RefCell<State>>) {
    let mut state = state.borrow_mut();
    if has_pending_network_request(&state) {
        // Abort the current request, but only when we know that we're
        // specifically querying for a network resource only.
        state.request = None;
    }
}

fn load_from_cache(state_rc: &Rc<RefCell<State>>) {
    debug_assert!(state_rc.borrow().request.is_none());
    let file_source = state_rc.borrow().file_source.clone();
    let Some(file_source) = file_source else {
        if let Some(tile) = tile_of(state_rc) {
            tile.set_error(get_cant_load_tile_error());
        }
        return;
    };

    if let Some(tile) = tile_of(state_rc) {
        tile.on_tile_action(TileOperation::RequestedFromCache);
    }
    let resource = {
        let mut state = state_rc.borrow_mut();
        state.resource.loading_method = LoadingMethod::CacheOnly;
        state.resource.clone()
    };
    let weak = Rc::downgrade(state_rc);
    let request = file_source.request(
        &resource,
        Box::new(move |res: Response| {
            let Some(state_rc) = weak.upgrade() else {
                return; // aborted
            };
            let tile = tile_of(&state_rc);
            let request = state_rc.borrow_mut().request.take();
            if let Some(tile) = &tile {
                tile.set_tried_cache();
            }
            let not_found = res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
            if not_found {
                // When the cache-only request could not be satisfied, don't
                // treat it as an error. A cache lookup could still return
                // data, _and_ an error, in particular when we were able to
                // find the data, but it is expired and the Cache-Control
                // headers indicated that we aren't allowed to use expired
                // responses. In this case, we still get the data which we can
                // use in our conditional network request.
                let mut state = state_rc.borrow_mut();
                state.resource.prior_modified = res.modified;
                state.resource.prior_expires = res.expires;
                state.resource.prior_etag = res.etag.clone();
                state.resource.prior_data = res.data.clone();
            } else {
                loaded_data(&state_rc, &res, LoadingMethod::CacheOnly);
            }
            let required = state_rc.borrow().necessity == TileNecessity::Required;
            if required {
                load_from_network(&state_rc);
            }
            drop(request);
        }),
    );
    let mut state = state_rc.borrow_mut();
    // A synchronous file source has answered (and cleared it) already.
    if state.request.is_none() {
        state.request = Some(request);
    }
}

fn loaded_data(state_rc: &Rc<RefCell<State>>, res: &Response, method: LoadingMethod) {
    let tile = tile_of(state_rc);
    if let Some(error) = &res.error {
        if error.reason != Reason::NotFound {
            if let Some(tile) = &tile {
                tile.set_error(RuntimeError::ptr(error.message.clone()));
                tile.on_tile_action(TileOperation::Error);
            }
            return;
        }
    }

    if let Some(tile) = &tile {
        if method == LoadingMethod::NetworkOnly {
            tile.on_tile_action(TileOperation::LoadFromNetwork);
        } else if method == LoadingMethod::CacheOnly {
            tile.on_tile_action(TileOperation::LoadFromCache);
        }
    }

    if res.not_modified {
        state_rc.borrow_mut().resource.prior_expires = res.expires;
        // Do not notify the tile; when we get this message, it already has
        // the current version of the data.
        if let Some(tile) = &tile {
            tile.set_metadata(res.modified, res.expires);
        }
    } else {
        {
            let mut state = state_rc.borrow_mut();
            state.resource.prior_modified = res.modified;
            state.resource.prior_expires = res.expires;
            state.resource.prior_etag = res.etag.clone();
        }
        if let Some(tile) = &tile {
            tile.set_metadata(res.modified, res.expires);
            let encoding = state_rc.borrow().encoding;
            tile.set_data(if res.no_content { None } else { res.data.clone() }, encoding);
        }
    }
}

fn load_from_network(state_rc: &Rc<RefCell<State>>) {
    debug_assert!(state_rc.borrow().request.is_none());
    let file_source = state_rc.borrow().file_source.clone();
    let Some(file_source) = file_source else {
        if let Some(tile) = tile_of(state_rc) {
            tile.set_error(get_cant_load_tile_error());
        }
        return;
    };

    if let Some(tile) = tile_of(state_rc) {
        tile.on_tile_action(TileOperation::RequestedFromNetwork);
    }
    // Instead of using Resource::LoadingMethod::All, we're first doing a
    // CacheOnly, and then a NetworkOnly request.
    let resource = {
        let mut state = state_rc.borrow_mut();
        state.resource.loading_method = LoadingMethod::NetworkOnly;
        state.resource.minimum_update_interval = state.update_parameters.minimum_update_interval;
        state.resource.storage_policy =
            if state.update_parameters.is_volatile { StoragePolicy::Volatile } else { StoragePolicy::Permanent };
        state.resource.clone()
    };
    let weak = Rc::downgrade(state_rc);
    let request = file_source.request(
        &resource,
        Box::new(move |res: Response| {
            let Some(state_rc) = weak.upgrade() else {
                return; // aborted
            };
            let request = state_rc.borrow_mut().request.take();
            loaded_data(&state_rc, &res, LoadingMethod::NetworkOnly);
            drop(request);
        }),
    );
    let mut state = state_rc.borrow_mut();
    if state.request.is_none() {
        state.request = Some(request);
    }
}
