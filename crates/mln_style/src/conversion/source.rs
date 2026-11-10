// Ports include/mln/style/conversion/source.hpp and
// src/mln/style/conversion/source.cpp.

use std::sync::Arc;

use geojson::GeoJSON;
use mln_style_types::SourceType;
use mln_util::constants;
use mln_util::tileset::{Tileset, VectorEncoding};

use super::{Converter, Convertible, Error};
use crate::source::Source;
use crate::sources::geojson_source::{GeoJSONOptions, GeoJSONSource};
use crate::sources::tile_source::{RasterSource, UrlOrTileset, VectorSource};

/// `convertURLOrTileset(value, error)`: a tile source can either specify a
/// URL to TileJSON, or inline TileJSON.
fn convert_url_or_tileset(value: &Convertible<'_>, error: &mut Error) -> Option<UrlOrTileset> {
    let Some(url_val) = value.object_member("url") else {
        let tileset = Tileset::convert(value, error)?;
        return Some(UrlOrTileset::Tileset(tileset));
    };

    let Some(url) = url_val.to_string() else {
        error.message = "source url must be a string".to_owned();
        return None;
    };

    Some(UrlOrTileset::Url(url))
}

/// `convertRasterSource(id, value, error)`.
fn convert_raster_source(id: &str, value: &Convertible<'_>, error: &mut Error) -> Option<Box<dyn Source>> {
    let url_or_tileset = convert_url_or_tileset(value, error)?;

    let mut tile_size = constants::TILE_SIZE_I;
    if let Some(tile_size_value) = value.object_member("tileSize") {
        match tile_size_value.to_number() {
            Some(size) if !(size < 0.0 || size > f32::from(u16::MAX)) => tile_size = size as u16,
            _ => {
                error.message = "invalid tileSize".to_owned();
                return None;
            }
        }
    }

    // Inline zooms win over the TileJSON's (RasterSource's setTilesetOverrides); an invalid one
    // rejects the source, as for vector sources
    let mut zooms = [None; 2];
    for (zoom, name) in zooms.iter_mut().zip(["minzoom", "maxzoom"]) {
        if let Some(zoom_value) = value.object_member(name) {
            match zoom_value.to_number() {
                Some(z) if !(z < 0.0 || z > f32::from(u8::MAX)) => *zoom = Some(z as u8),
                _ => {
                    error.message = format!("invalid {name}");
                    return None;
                }
            }
        }
    }

    Some(Box::new(RasterSource::new(id, url_or_tileset, tile_size, SourceType::Raster, zooms[0], zooms[1])))
}

// dem-heatmap
/// `convertRasterDEMSource(id, value, error)`.
fn convert_raster_dem_source(id: &str, value: &Convertible<'_>, error: &mut Error) -> Option<Box<dyn Source>> {
    let url_or_tileset = convert_url_or_tileset(value, error)?;

    let mut tile_size = constants::TILE_SIZE_I;
    if let Some(tile_size_value) = value.object_member("tileSize") {
        match tile_size_value.to_number() {
            Some(size) if !(size < 0.0 || size > f32::from(u16::MAX)) => tile_size = size as u16,
            _ => {
                error.message = "invalid tileSize".to_owned();
                return None;
            }
        }
    }

    // TODO: Refactor TileJSON parsing responsibilities. Currently `Tileset`
    // handles full TileJSON parsing with optional parameters. Since
    // style.json can contain TileJSON parameter overrides or non-TileJSON
    // spec parameters, a separate mechanism is needed. Ideally these two
    // pathways share parsing logic.
    //
    // An invalid `encoding`, `minzoom` or `maxzoom` rejects the source, as for vector sources:
    // building it without the options decoded a terrarium DEM as Mapbox, with wrong heights and
    // nothing logged. No options at all is not an error.
    let mut options_error = Error::default();
    let options = crate::conversion::tileset::SourceOptions::convert(value, &mut options_error);
    if options.is_none() && !options_error.message.is_empty() {
        *error = options_error;
        return None;
    }

    Some(Box::new(crate::sources::tile_source::RasterDEMSource::new(id, url_or_tileset, tile_size, options)))
}

/// `convertVectorSource(id, value, error)`.
fn convert_vector_source(id: &str, value: &Convertible<'_>, error: &mut Error) -> Option<Box<dyn Source>> {
    let url_or_tileset = convert_url_or_tileset(value, error)?;
    let mut maxzoom = None;
    if let Some(maxzoom_value) = value.object_member("maxzoom") {
        match maxzoom_value.to_number() {
            Some(z) if !(z < 0.0 || z > f32::from(u8::MAX)) => maxzoom = Some(z),
            _ => {
                error.message = "invalid maxzoom".to_owned();
                return None;
            }
        }
    }
    let mut minzoom = None;
    if let Some(minzoom_value) = value.object_member("minzoom") {
        match minzoom_value.to_number() {
            Some(z) if !(z < 0.0 || z > f32::from(u8::MAX)) => minzoom = Some(z),
            _ => {
                error.message = "invalid minzoom".to_owned();
                return None;
            }
        }
    }
    let mut encoding = VectorEncoding::Mapbox;
    if let Some(encoding_value) = value.object_member("encoding") {
        match encoding_value.to_string().as_deref() {
            Some("mvt") => encoding = VectorEncoding::Mapbox,
            Some("mlt") => encoding = VectorEncoding::MLT,
            _ => {
                error.message = "invalid encoding".to_owned();
                return None;
            }
        }
    }
    Some(Box::new(VectorSource::new(id, url_or_tileset, maxzoom, minzoom, encoding)))
}

/// `convertGeoJSONSource(id, value, error)`.
fn convert_geojson_source(id: &str, value: &Convertible<'_>, error: &mut Error) -> Option<Box<dyn Source>> {
    let Some(data_value) = value.object_member("data") else {
        error.message = "GeoJSON source must have a data value".to_owned();
        return None;
    };

    let mut options = GeoJSONOptions::default_options();
    if let Some(converted) = GeoJSONOptions::convert(value, error) {
        options = Arc::new(converted);
    }

    let result = GeoJSONSource::new(id, options);

    if data_value.is_object() {
        let geojson = GeoJSON::convert(&data_value, error)?;
        if let Err(e) = result.set_geojson(&geojson) {
            // TODO(port): C++ lets the Supercluster exception (a clustered
            // non-point feature) propagate out of the style parse.
            error.message = e;
            return None;
        }
    } else if let Some(url) = data_value.to_string() {
        result.set_url(&url);
    } else {
        error.message = "GeoJSON data must be a URL or an object".to_owned();
        return None;
    }

    Some(Box::new(result))
}

/// `convertImageSource(id, value, error)` (conformance).
fn convert_image_source(id: &str, value: &Convertible<'_>, error: &mut Error) -> Option<Box<dyn Source>> {
    let Some(url_value) = value.object_member("url") else {
        error.message = "Image source must have a url value".to_owned();
        return None;
    };

    let Some(url_string) = url_value.to_string() else {
        error.message = "Image url must be a URL string".to_owned();
        return None;
    };

    let Some(coordinates_value) = value.object_member("coordinates") else {
        error.message = "Image source must have a coordinates values".to_owned();
        return None;
    };

    if !coordinates_value.is_array() || coordinates_value.array_length() != 4 {
        error.message = "Image coordinates must be an array of four longitude latitude pairs".to_owned();
        return None;
    }

    let mut coordinates = [mln_util::LatLng::default(); 4];
    for (i, coordinate) in coordinates.iter_mut().enumerate() {
        let lat_lng = super::convert::<mln_util::LatLng>(&coordinates_value.array_member(i), error)?;
        *coordinate = lat_lng;
    }
    let result = crate::sources::image_source::ImageSource::new(id, coordinates);
    result.set_url(&url_string);

    Some(Box::new(result))
}

/// `convert<std::unique_ptr<Source>>(value, error, id)`.
pub fn convert_source(value: &Convertible<'_>, error: &mut Error, id: &str) -> Option<Box<dyn Source>> {
    if !value.is_object() {
        error.message = "source must be an object".to_owned();
        return None;
    }

    let Some(type_value) = value.object_member("type") else {
        error.message = "source must have a type".to_owned();
        return None;
    };

    let Some(type_) = type_value.to_string() else {
        error.message = "source type must be a string".to_owned();
        return None;
    };
    match type_.as_str() {
        "raster" => convert_raster_source(id, value, error),
        "vector" => convert_vector_source(id, value, error),
        "geojson" => convert_geojson_source(id, value, error),
        "raster-dem" => convert_raster_dem_source(id, value, error), // dem-heatmap
        "image" => convert_image_source(id, value, error),           // conformance
        _ => {
            error.message = "invalid source type".to_owned();
            None
        }
    }
}
