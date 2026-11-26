//! Custom vector sources, whose tiles' encoded bytes the host gives on
//! demand. As for a custom geometry source, the host answers its tile
//! functions through the [`CustomVectorSourceHandle`] they get.

use std::sync::Arc;

pub use super::custom::{CustomVectorSourceHandle, CustomVectorTileData, TileDataFormat};
use super::custom::{CustomVectorSourceOptions, CustomVectorSourceType, TileFunction, TileFunctions};
use crate::range::Range;
use crate::source::{Source, SourceData};

#[derive(Clone)]
pub struct Options {
    /// Asks the host for a tile's data, which it gives with
    /// [`CustomVectorSourceHandle::set_tile_data`].
    pub fetch_tile_function: Option<TileFunction<CustomVectorTileData>>,
    /// Tells the host that no tile needs a tile's data any more.
    pub cancel_tile_function: Option<TileFunction<CustomVectorTileData>>,
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

/// The custom vector source `id`.
#[must_use]
pub fn create(id: &str, options: Options) -> Source {
    let tile_functions =
        TileFunctions { fetch_tile: options.fetch_tile_function, cancel_tile: options.cancel_tile_function };
    let options =
        CustomVectorSourceOptions { zoom_range: options.zoom_range, tile_functions: Arc::new(tile_functions) };
    Source::new(id, SourceData::<CustomVectorSourceType>::new(options))
}
