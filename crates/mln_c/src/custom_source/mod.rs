//! The C API's custom sources, `custom-geometry` and `custom-vector`:
//! source types of this crate's own, whose tiles' data the host gives on
//! demand. A source holds the host's tile functions, which its origin's
//! provider calls on a pool thread with a [`CustomSourceHandle`] that the
//! host answers through. The runtimes that the C API makes register both
//! types ([`registry`]).

use std::sync::{Arc, LazyLock};

use mln_engine::{Registry, RegistryBuilder, register_builtins};
use mln_style::error::Error;
use mln_style::geo::LatLngBounds;
use mln_style::tile_id::CanonicalTileID;

mod geometry;
mod origin;
mod provider;
mod vector;

pub(crate) use geometry::{CustomGeometrySourceOptions, CustomGeometrySourceType, TileOptions};
pub(crate) use vector::{CustomVectorSourceOptions, CustomVectorSourceType, CustomVectorTileData, TileDataFormat};

/// The registry of the C API's runtimes and tile workers: the built-ins and
/// the custom source types.
pub(crate) fn registry() -> Arc<Registry> {
    static REGISTRY: LazyLock<Arc<Registry>> = LazyLock::new(|| {
        let mut builder = RegistryBuilder::new();
        // Into an empty builder, the built-ins and the custom source types,
        // whose names differ, register.
        let registered = register_builtins(&mut builder)
            .and_then(|()| builder.add_source_type::<CustomGeometrySourceType>())
            .and_then(|()| builder.add_source_type::<CustomVectorSourceType>());
        debug_assert_eq!(registered, Ok(()));
        builder.build()
    });
    Arc::clone(&REGISTRY)
}

/// Called with the canonical ID of a tile that the map needs, or no longer
/// needs, and the handle that the host answers through. It runs on a pool
/// thread, one call at a time, in the order the tiles asked, so a host may
/// answer from inside it: the answer takes effect after the call.
pub(crate) type TileFunction<D> = Arc<dyn Fn(&CanonicalTileID, &CustomSourceHandle<D>) + Send + Sync>;

/// A custom source's tile functions, which take answers of type `D`.
pub(crate) struct TileFunctions<D> {
    /// Asks the host for a tile's data, which it gives with
    /// [`CustomSourceHandle::set_tile_data`].
    pub(crate) fetch_tile: Option<TileFunction<D>>,
    /// Tells the host that no tile needs a tile's data any more.
    pub(crate) cancel_tile: Option<TileFunction<D>>,
}

impl<D> std::fmt::Debug for TileFunctions<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileFunctions")
            .field("fetch_tile", &self.fetch_tile.is_some())
            .field("cancel_tile", &self.cancel_tile.is_some())
            .finish()
    }
}

/// What a custom source's handle reaches: the provider of the source's
/// tiles.
pub(crate) trait CustomTileResponder<D>: Send + Sync {
    /// Hands over the tile's data. It is dropped unless a tile needs it.
    fn set_tile_data(&self, tile_id: CanonicalTileID, data: D);
    /// Reports that the host failed to load the tile.
    fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error);
    /// Drops the tile's data, and asks the host for it again if a tile needs
    /// it.
    fn invalidate_tile(&self, tile_id: CanonicalTileID);
    /// Invalidates every tile that `bounds` intersect, at its own zoom.
    fn invalidate_region(&self, bounds: LatLngBounds);
    /// Whether the provider is gone, so that the calls above do nothing.
    fn is_closed(&self) -> bool;
}

/// A `Send` handle to the provider of a custom source's tiles,
/// which the provider hands to the host's tile functions. The host answers
/// through it, from any thread; it does nothing once the provider is gone.
/// Clones reach the same provider.
pub(crate) struct CustomSourceHandle<D>(Arc<dyn CustomTileResponder<D>>);

impl<D> Clone for CustomSourceHandle<D> {
    fn clone(&self) -> Self {
        CustomSourceHandle(Arc::clone(&self.0))
    }
}

impl<D> std::fmt::Debug for CustomSourceHandle<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomSourceHandle").finish_non_exhaustive()
    }
}

impl<D> CustomSourceHandle<D> {
    /// The handle of the provider behind `responder`.
    #[must_use]
    pub(crate) fn new(responder: Arc<dyn CustomTileResponder<D>>) -> Self {
        CustomSourceHandle(responder)
    }

    /// Hands over the tile's data. The source ignores data for a tile it
    /// hasn't asked for, or has since cancelled.
    pub(crate) fn set_tile_data(&self, tile_id: CanonicalTileID, data: D) {
        self.0.set_tile_data(tile_id, data);
    }

    /// Reports that the host failed to load the tile. Invalidating the tile,
    /// or needing it again, asks for it again.
    pub(crate) fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        self.0.set_tile_error(tile_id, error);
    }

    /// Drops the tile's data, and asks the host for it again if a tile needs
    /// it.
    pub(crate) fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        self.0.invalidate_tile(tile_id);
    }

    /// Invalidates every tile that `bounds` intersect.
    pub(crate) fn invalidate_region(&self, bounds: LatLngBounds) {
        self.0.invalidate_region(bounds);
    }

    /// Whether the provider is gone, so that the handle does nothing.
    #[must_use]
    pub(crate) fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    /// Whether both handles reach the same provider.
    #[must_use]
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
