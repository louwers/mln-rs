use std::sync::Arc;

use geojson::GeoJSON;
use mln_style_types::SourceType;
use mln_util::constants;

use super::tileset::{convert_tileset, optional_member};
use super::{Converter, Convertible, Error};
use crate::source::Source;
use crate::sources::geojson_source::{GeoJSONOptions, GeoJSONSource};
use crate::sources::tile_source::TileSource;

/// A raster, raster-dem or vector source: the TileJSON at its `url`, its own TileJSON members, or
/// both, its members winning.
fn convert_tile_source(id: &str, value: &Convertible<'_>, source_type: SourceType) -> Result<Box<dyn Source>, String> {
    let url = optional_member(value, "url", Convertible::to_string)?;
    let tileset = convert_tileset(value, source_type)?;
    if url.is_none() && tileset.tiles.is_none() {
        return Err("source must have a url or tiles".to_owned());
    }
    let tile_size = optional_member(value, "tileSize", |size| {
        size.to_number().filter(|size| (0.0..=f32::from(u16::MAX)).contains(size)).map(|size| size as u16)
    })?
    .unwrap_or(constants::TILE_SIZE_I);
    Ok(Box::new(match source_type {
        SourceType::Raster => TileSource::raster(id, url, tileset, tile_size),
        SourceType::RasterDEM => TileSource::raster_dem(id, url, tileset, tile_size),
        _ => TileSource::vector(id, url, tileset),
    }))
}

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
            // C++ lets the Supercluster exception (a clustered
            // non-point feature) propagate out of the style parse.
            error.message = e.message;
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

fn convert_image_source(id: &str, value: &Convertible<'_>, error: &mut Error) -> Option<Box<dyn Source>> {
    // A source without a url starts empty, until it gets an image or a url.
    let url_string = match value.object_member("url") {
        Some(url_value) => {
            let Some(url_string) = url_value.to_string() else {
                error.message = "Image url must be a URL string".to_owned();
                return None;
            };
            Some(url_string)
        }
        None => None,
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
    if let Some(url_string) = url_string {
        result.set_url(&url_string);
    }

    Some(Box::new(result))
}

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
    let tile_source_type = match type_.as_str() {
        "raster" => SourceType::Raster,
        "vector" => SourceType::Vector,
        "raster-dem" => SourceType::RasterDEM, // dem-heatmap
        "geojson" => return convert_geojson_source(id, value, error),
        "image" => return convert_image_source(id, value, error), // conformance
        _ => {
            error.message = "invalid source type".to_owned();
            return None;
        }
    };
    convert_tile_source(id, value, tile_source_type).map_err(|message| error.message = message).ok()
}
