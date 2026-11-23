//! Custom vector sources, whose tiles' encoded bytes the host gives on
//! demand. As for a custom geometry source, the host answers its tile
//! functions through a [`CustomVectorSourceHandle`].

use std::sync::{Arc, Weak};

use mln_storage::Data;
use mln_style::source_type::{Source, SourceData};
use mln_tile_id::CanonicalTileID;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::pool::Pool;
use mln_util::range::Range;

use super::provider::{CustomProvider, TileFunction};
use super::{CustomVectorProvider, CustomVectorSourceOptions, CustomVectorSourceType, TileDataFormat};

#[derive(Clone)]
pub struct Options {
    /// Asks the host for a tile's data, which it gives with
    /// [`CustomVectorSourceHandle::set_tile_data`].
    pub fetch_tile_function: Option<TileFunction>,
    /// Tells the host that no tile needs a tile's data any more.
    pub cancel_tile_function: Option<TileFunction>,
    pub zoom_range: Range<u8>,
}

impl Default for Options {
    fn default() -> Self {
        Options { fetch_tile_function: None, cancel_tile_function: None, zoom_range: Range::new(0, 18) }
    }
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("fetch_tile_function", &self.fetch_tile_function.is_some())
            .field("cancel_tile_function", &self.cancel_tile_function.is_some())
            .field("zoom_range", &self.zoom_range)
            .finish()
    }
}

/// A `Send` handle to a custom vector source's provider, for the host to
/// answer its tile functions from any thread. It does nothing once the
/// source is gone.
#[derive(Clone)]
pub struct CustomVectorSourceHandle {
    provider: Weak<CustomVectorProvider>,
}

impl std::fmt::Debug for CustomVectorSourceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomVectorSourceHandle").finish_non_exhaustive()
    }
}

impl CustomVectorSourceHandle {
    /// The handle of `source`, if it's a custom vector source.
    #[must_use]
    pub fn of(source: &Source) -> Option<Self> {
        SourceData::recover(source).map(Self::new)
    }

    pub(crate) fn new(data: &SourceData<CustomVectorSourceType>) -> Self {
        CustomVectorSourceHandle { provider: Arc::downgrade(&data.options.provider) }
    }

    /// Hands over the tile's encoded bytes. `None` is a tile without data.
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: Option<Data>, format: TileDataFormat) {
        if let Some(provider) = self.provider.upgrade() {
            provider.set_tile_data(tile_id, (data, format));
        }
    }

    /// Reports that the host failed to load the tile. Invalidating the tile, or
    /// needing it again, asks for it again.
    pub fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        if let Some(provider) = self.provider.upgrade() {
            provider.set_tile_error(tile_id, error);
        }
    }

    /// Drops the tile's data, and asks the host for it again if a tile needs
    /// it.
    pub fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        if let Some(provider) = self.provider.upgrade() {
            provider.invalidate_tile(tile_id);
        }
    }

    /// Invalidates every tile that `bounds` intersect.
    pub fn invalidate_region(&self, bounds: LatLngBounds) {
        if let Some(provider) = self.provider.upgrade() {
            provider.invalidate_region(bounds);
        }
    }
}

/// The custom vector source `id`, whose tile functions run on `pool`.
#[must_use]
pub fn create(id: &str, options: Options, pool: &Pool) -> Source {
    let provider = CustomProvider::new(options.fetch_tile_function, options.cancel_tile_function, pool);
    let options = CustomVectorSourceOptions { zoom_range: options.zoom_range, provider };
    Source::new(id, SourceData::<CustomVectorSourceType>::new(options))
}
