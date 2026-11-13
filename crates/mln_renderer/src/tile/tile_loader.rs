// C++ `TileLoader<T>` holds `T& tile` and request callbacks capture `this`.
// Here the loader state is an `Rc<RefCell<..>>` the callbacks reach through
// a `Weak` (a dropped loader = C++ `shared->aborted`), and the tile is the
// GeometryTile's render-thread state.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use mln_storage::response::Reason;
use mln_storage::{AsyncRequest, Data, FileSource, LoadingMethod, Resource, Response, StoragePolicy};
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::tileset::Tileset;

use crate::tile::geometry_tile::GeometryTileInner;
use crate::tile::geometry_tile_worker::SharedTileData;
use crate::tile::tile::{TileLoadOrder, TileNecessity, TileOperation, TileUpdateParameters};
use crate::tile_parameters::TileParameters;

fn get_cant_load_tile_error() -> Error {
    Error::native("Can't load tile.")
}

/// The tile data encoding of a vector tile: `VectorMVTTile` or `VectorMLTTile`
/// (raster tiles ignore it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VectorTileEncoding {
    Mvt,
    /// MLT, decoded with `fast_pfor_enabled`.
    Mlt {
        fast_pfor_enabled: bool,
    },
}

/// What a loader does with the tile it loads (`T` in C++).
pub(crate) trait LoaderTile {
    fn set_error(&self, err: Error);
    /// Makes the tile wait for data again.
    fn set_loading(&self);
    fn set_tried_cache(&self);
    fn on_tile_action(&self, op: TileOperation);
    fn set_metadata(&self, modified: Option<mln_util::chrono::Timestamp>, expires: Option<mln_util::chrono::Timestamp>);
    /// Sets the tile's data. Without data, the tile is in the tileset and has
    /// nothing in it: it's renderable and draws nothing.
    fn set_data(&self, data: Option<Data>, encoding: VectorTileEncoding);
    /// Loads the tile without data for a tile the tileset doesn't have at this zoom level. The
    /// tile isn't renderable, so the pyramid draws a tile from another zoom level in its place.
    fn set_missing(&self, encoding: VectorTileEncoding);
    /// Whether a tile the tileset doesn't have (a 404) loads as an empty tile, as a vector tile
    /// does in GL JS, rather than as a missing one, as a raster tile does.
    fn not_found_is_empty(&self) -> bool {
        false
    }
}

/// `VectorTile`'s half of the loader interface (`vector_tile.cpp`,
/// `vector_mvt_tile.cpp`, `vector_mlt_tile.cpp`).
impl LoaderTile for GeometryTileInner {
    fn set_error(&self, err: Error) {
        GeometryTileInner::set_error(self, err);
    }

    fn set_loading(&self) {
        self.base.loaded.set(false);
    }

    fn set_tried_cache(&self) {
        self.base.set_tried_cache();
    }

    fn on_tile_action(&self, op: TileOperation) {
        self.base.on_tile_action(op);
    }

    fn set_metadata(
        &self,
        modified: Option<mln_util::chrono::Timestamp>,
        expires: Option<mln_util::chrono::Timestamp>,
    ) {
        self.base.modified.set(modified);
        self.base.expires.set(expires);
    }

    fn set_data(&self, data: Option<Data>, encoding: VectorTileEncoding) {
        if !self.is_obsolete() {
            self.missing.set(false);
            let data: Option<SharedTileData> = data.map(|data| match encoding {
                VectorTileEncoding::Mvt => Arc::new(mln_tile::VectorMVTTileData::new(data)) as SharedTileData,
                VectorTileEncoding::Mlt { fast_pfor_enabled } => {
                    Arc::new(mln_tile::VectorMLTTileData::new(data, fast_pfor_enabled)) as SharedTileData
                }
            });
            GeometryTileInner::set_data(self, data);
        }
    }

    fn set_missing(&self, _encoding: VectorTileEncoding) {
        if !self.is_obsolete() {
            self.missing.set(true);
            GeometryTileInner::set_data(self, None);
        }
    }

    fn not_found_is_empty(&self) -> bool {
        true
    }
}

struct State {
    tile: Weak<dyn LoaderTile>,
    encoding: VectorTileEncoding,
    sparse: bool,
    necessity: TileNecessity,
    resource: Resource,
    file_source: Option<Arc<dyn FileSource>>,
    request: Option<Box<dyn AsyncRequest>>,
    /// The last response was an error: nothing requests the tile again until
    /// `retry_failed_load`.
    failed: bool,
    update_parameters: TileUpdateParameters,
}

pub(crate) struct TileLoader {
    state: Rc<RefCell<State>>,
}

fn tile_of(state: &Rc<RefCell<State>>) -> Option<Rc<dyn LoaderTile>> {
    state.borrow().tile.upgrade()
}

impl TileLoader {
    pub(crate) fn new(
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
            sparse: tileset.sparse,
            necessity: TileNecessity::Optional,
            resource,
            file_source: if tileset.tiles.is_empty() { None } else { parameters.file_source.clone() },
            request: None,
            failed: false,
            update_parameters: TileUpdateParameters { minimum_update_interval: Duration::ZERO, is_volatile: false },
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

    pub(crate) fn set_necessity(&self, new_necessity: TileNecessity) {
        let changed = {
            let mut state = self.state.borrow_mut();
            if new_necessity == state.necessity {
                false
            } else {
                state.necessity = new_necessity;
                true
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

    /// Sets when the tile's network request starts among those waiting. The
    /// requests share the resource's order, so one that waits now moves.
    pub(crate) fn set_load_order(&self, order: TileLoadOrder) {
        self.state.borrow().resource.order.set(order.value());
    }

    pub(crate) fn set_update_parameters(&self, params: &TileUpdateParameters) {
        let reload = {
            let mut state = self.state.borrow_mut();
            if state.update_parameters == *params {
                false
            } else {
                state.update_parameters = *params;
                if has_pending_network_request(&state) {
                    // Update the pending request.
                    state.request = None;
                    true
                } else {
                    false
                }
            }
        };
        if reload {
            load_from_network(&self.state);
        }
    }

    /// Requests a required tile whose last response was an error again, and
    /// makes it wait for the answer.
    pub(crate) fn retry_failed_load(&self) {
        {
            let mut state = self.state.borrow_mut();
            if !state.failed || state.request.is_some() || state.necessity != TileNecessity::Required {
                return;
            }
            state.failed = false;
        }
        if let Some(tile) = tile_of(&self.state) {
            tile.set_loading();
        }
        load_from_network(&self.state);
    }
}

fn has_pending_network_request(state: &State) -> bool {
    state.resource.loading_method == LoadingMethod::NetworkOnly && state.request.is_some()
}

/// Called when the tile is one of the ideal tiles that we want to show
/// definitely. The tile source should try to make every effort (e.g. fetch
/// from internet, or revalidate existing resources).
fn make_required(state: &Rc<RefCell<State>>) {
    if state.borrow().request.is_none() {
        load_from_network(state);
    }
}

/// Called when the zoom level no longer corresponds to the displayed one, but
/// we're still interested in retaining the tile, e.g. for backfill.
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
    if let Some(error) = &res.error
        && error.reason != Reason::NotFound
    {
        state_rc.borrow_mut().failed = true;
        if let Some(tile) = &tile {
            tile.set_error(Error::native(error.message.clone()));
            tile.on_tile_action(TileOperation::Error);
        }
        return;
    }
    state_rc.borrow_mut().failed = false;

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
            let (encoding, sparse) = {
                let state = state_rc.borrow();
                (state.encoding, state.sparse)
            };
            // A 404 (Not Found), unlike an empty response (204, or no tile in an archive), says
            // the tileset has no tile here.
            let not_found = res.tile_not_found || res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
            let empty = res.no_content || not_found;
            if empty && (sparse || (not_found && !tile.not_found_is_empty())) {
                tile.set_missing(encoding);
            } else {
                tile.set_data(if empty { None } else { res.data.clone() }, encoding);
            }
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
