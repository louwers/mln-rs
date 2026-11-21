//! A runtime map's commands for its custom sources.

use std::sync::{Arc, Weak};

use geojson::GeoJson;
use mln_runtime::{Applied, Completion, Map};
use mln_style::source_type::Source;
use mln_tile_id::CanonicalTileID;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::range::Range;

use super::geometry::{self, CustomGeometrySourceHandle};
use super::provider::TileFunction;
use super::vector::{self, CustomVectorSourceHandle};
use super::{TileDataFormat, TileOptions};
use crate::geojson::parse_geojson;
use crate::not_a;

/// The callbacks of a custom source, which run on a library thread. A map
/// asks for a tile with `fetch_tile`, and answers come back through the
/// source's commands, such as
/// [`CustomSourceCommands::set_custom_geometry_source_tile_data`].
/// `cancel_tile` reports that the map no longer needs a tile.
///
/// The callbacks drop once the source has left the style and no call is
/// running, so the source's last call happens before then.
pub struct CustomSourceCallbacks {
    pub fetch_tile: Box<dyn Fn(CanonicalTileID) + Send + Sync>,
    pub cancel_tile: Option<Box<dyn Fn(CanonicalTileID) + Send + Sync>>,
}

impl std::fmt::Debug for CustomSourceCallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomSourceCallbacks")
            .field("cancel_tile", &self.cancel_tile.is_some())
            .finish_non_exhaustive()
    }
}

/// The provider's tile functions over `callbacks`, which hold them weakly,
/// so that they drop when the map lets go of the source rather than when
/// the source's provider does.
fn tile_functions(callbacks: &Arc<CustomSourceCallbacks>) -> (Option<TileFunction>, Option<TileFunction>) {
    let fetch: Weak<CustomSourceCallbacks> = Arc::downgrade(callbacks);
    let fetch = Arc::new(move |tile: &CanonicalTileID| {
        if let Some(callbacks) = fetch.upgrade() {
            (callbacks.fetch_tile)(*tile);
        }
    });
    let cancel = callbacks.cancel_tile.as_ref().map(|_| {
        let cancel = Arc::downgrade(callbacks);
        let cancel: TileFunction = Arc::new(move |tile: &CanonicalTileID| {
            if let Some(callback) = cancel.upgrade().as_ref().and_then(|callbacks| callbacks.cancel_tile.as_ref()) {
                callback(*tile);
            }
        });
        cancel
    });
    (Some(fetch), cancel)
}

/// The features a custom geometry tile draws: its provider draws only a
/// feature collection, so a lone feature or geometry becomes one.
fn feature_collection(geojson: GeoJson) -> GeoJson {
    match geojson {
        GeoJson::FeatureCollection(_) => geojson,
        GeoJson::Feature(feature) => GeoJson::from(vec![feature]),
        GeoJson::Geometry(geometry) => GeoJson::from(vec![geojson::Feature::from(geometry)]),
    }
}

fn custom_geometry(source: &Source) -> Result<CustomGeometrySourceHandle, Error> {
    CustomGeometrySourceHandle::of(source).ok_or_else(|| not_a(source.id(), "a custom geometry source"))
}

fn custom_vector(source: &Source) -> Result<CustomVectorSourceHandle, Error> {
    CustomVectorSourceHandle::of(source).ok_or_else(|| not_a(source.id(), "a custom vector source"))
}

/// A runtime map's commands for its custom geometry and custom vector
/// sources. A command for a source of the other kind completes with
/// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind), and one for a
/// source the style doesn't have with `NotFound`.
pub trait CustomSourceCommands {
    /// Adds a source whose GeoJSON tiles come from `callbacks`. The map
    /// holds the callbacks until the source leaves the style, by its
    /// removal, a style load or the map's release, and drops them before
    /// the command that removed it completes. A command that fails drops
    /// them before it completes.
    fn add_custom_geometry_source(
        &self,
        source_id: impl Into<String>,
        callbacks: CustomSourceCallbacks,
        zoom_range: Range<u8>,
        tile_options: TileOptions,
    ) -> Result<Completion<Applied>, Error>;

    /// Adds a source whose vector tiles come from `callbacks`, which the map
    /// holds as [`add_custom_geometry_source`](Self::add_custom_geometry_source)
    /// says.
    fn add_custom_vector_source(
        &self,
        source_id: impl Into<String>,
        callbacks: CustomSourceCallbacks,
        zoom_range: Range<u8>,
    ) -> Result<Completion<Applied>, Error>;

    /// Sets the data of a tile of a custom geometry source from GeoJSON text:
    /// a feature collection, feature or geometry. The source ignores data
    /// for a tile it hasn't asked for, or has since cancelled.
    fn set_custom_geometry_source_tile_data(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error>;

    /// Reports that a tile of a custom geometry source failed to load.
    fn set_custom_geometry_source_tile_error(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
        message: impl Into<String>,
    ) -> Result<Completion<Applied>, Error>;

    /// Makes a custom geometry source ask for a tile again when the map needs
    /// it.
    fn invalidate_custom_geometry_source_tile(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
    ) -> Result<Completion<Applied>, Error>;

    /// Makes a custom geometry source ask again for the tiles in `bounds`
    /// that the map still needs.
    fn invalidate_custom_geometry_source_region(
        &self,
        source_id: impl Into<String>,
        bounds: LatLngBounds,
    ) -> Result<Completion<Applied>, Error>;

    /// Sets the data of a tile of a custom vector source: its bytes in
    /// `format`, or `None` for a tile without data. The source ignores data
    /// for a tile it hasn't asked for, or has since cancelled.
    fn set_custom_vector_source_tile_data(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
        data: Option<Vec<u8>>,
        format: TileDataFormat,
    ) -> Result<Completion<Applied>, Error>;

    /// Reports that a tile of a custom vector source failed to load.
    fn set_custom_vector_source_tile_error(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
        message: impl Into<String>,
    ) -> Result<Completion<Applied>, Error>;

    /// Makes a custom vector source ask for a tile again when the map needs
    /// it.
    fn invalidate_custom_vector_source_tile(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
    ) -> Result<Completion<Applied>, Error>;

    /// Makes a custom vector source ask again for the tiles in `bounds` that
    /// the map still needs.
    fn invalidate_custom_vector_source_region(
        &self,
        source_id: impl Into<String>,
        bounds: LatLngBounds,
    ) -> Result<Completion<Applied>, Error>;
}

impl CustomSourceCommands for Map {
    fn add_custom_geometry_source(
        &self,
        source_id: impl Into<String>,
        callbacks: CustomSourceCallbacks,
        zoom_range: Range<u8>,
        tile_options: TileOptions,
    ) -> Result<Completion<Applied>, Error> {
        let (source_id, callbacks) = (source_id.into(), Arc::new(callbacks));
        let (fetch_tile_function, cancel_tile_function) = tile_functions(&callbacks);
        let options = geometry::Options { fetch_tile_function, cancel_tile_function, zoom_range, tile_options };
        self.add_style_source_keeping(
            move |map| {
                geometry::create(&source_id, options, map.pool())
                    .map_err(|error| Error::invalid_argument(error.to_string()))
            },
            Box::new(callbacks),
        )
    }

    fn add_custom_vector_source(
        &self,
        source_id: impl Into<String>,
        callbacks: CustomSourceCallbacks,
        zoom_range: Range<u8>,
    ) -> Result<Completion<Applied>, Error> {
        let (source_id, callbacks) = (source_id.into(), Arc::new(callbacks));
        let (fetch_tile_function, cancel_tile_function) = tile_functions(&callbacks);
        let options = vector::Options { fetch_tile_function, cancel_tile_function, zoom_range };
        self.add_style_source_keeping(
            move |map| Ok(vector::create(&source_id, options, map.pool())),
            Box::new(callbacks),
        )
    }

    fn set_custom_geometry_source_tile_data(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let json = json.into();
        self.command_style_source(source_id, move |source| {
            let geojson = feature_collection(parse_geojson(&json)?);
            custom_geometry(source)?.set_tile_data(tile_id, geojson);
            Ok(())
        })
    }

    fn set_custom_geometry_source_tile_error(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
        message: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let message = message.into();
        self.command_style_source(source_id, move |source| {
            custom_geometry(source)?.set_tile_error(tile_id, Error::native(message));
            Ok(())
        })
    }

    fn invalidate_custom_geometry_source_tile(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
    ) -> Result<Completion<Applied>, Error> {
        self.command_style_source(source_id, move |source| {
            custom_geometry(source)?.invalidate_tile(tile_id);
            Ok(())
        })
    }

    fn invalidate_custom_geometry_source_region(
        &self,
        source_id: impl Into<String>,
        bounds: LatLngBounds,
    ) -> Result<Completion<Applied>, Error> {
        self.command_style_source(source_id, move |source| {
            custom_geometry(source)?.invalidate_region(bounds);
            Ok(())
        })
    }

    fn set_custom_vector_source_tile_data(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
        data: Option<Vec<u8>>,
        format: TileDataFormat,
    ) -> Result<Completion<Applied>, Error> {
        self.command_style_source(source_id, move |source| {
            custom_vector(source)?.set_tile_data(tile_id, data.map(Arc::new), format);
            Ok(())
        })
    }

    fn set_custom_vector_source_tile_error(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
        message: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let message = message.into();
        self.command_style_source(source_id, move |source| {
            custom_vector(source)?.set_tile_error(tile_id, Error::native(message));
            Ok(())
        })
    }

    fn invalidate_custom_vector_source_tile(
        &self,
        source_id: impl Into<String>,
        tile_id: CanonicalTileID,
    ) -> Result<Completion<Applied>, Error> {
        self.command_style_source(source_id, move |source| {
            custom_vector(source)?.invalidate_tile(tile_id);
            Ok(())
        })
    }

    fn invalidate_custom_vector_source_region(
        &self,
        source_id: impl Into<String>,
        bounds: LatLngBounds,
    ) -> Result<Completion<Applied>, Error> {
        self.command_style_source(source_id, move |source| {
            custom_vector(source)?.invalidate_region(bounds);
            Ok(())
        })
    }
}
