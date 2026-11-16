use std::sync::Arc;

use geojson::GeoJson;

use super::geojson_options::convert_geojson_options;
use super::{Converter, Convertible, Error};
use crate::StyleRegistry;
use crate::expression::Functions;
use crate::source::Source;
use crate::sources::geojson_source::{GeoJSONOptions, GeoJSONSource};

pub(crate) fn convert_geojson_source(
    id: &str,
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
    error: &mut Error,
) -> Option<Box<dyn Source>> {
    let Some(data_value) = value.object_member("data") else {
        error.message = "GeoJSON source must have a data value".to_owned();
        return None;
    };

    let mut options = GeoJSONOptions::default_options();
    if let Some(converted) = convert_geojson_options(value, functions, error) {
        options = Arc::new(converted);
    }

    let result = GeoJSONSource::new(id, options);

    if data_value.is_object() {
        let geojson = GeoJson::convert(&data_value, error)?;
        if let Err(e) = result.set_geojson(geojson) {
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

pub(crate) fn convert_image_source(id: &str, value: &Convertible<'_>, error: &mut Error) -> Option<Box<dyn Source>> {
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

/// Converts the source `id` through its type in `registry`. A GeoJSON
/// source's `clusterProperties` expressions parse against the registry's
/// functions.
pub fn convert_source(
    value: &Convertible<'_>,
    error: &mut Error,
    id: &str,
    registry: &StyleRegistry,
) -> Option<Box<dyn Source>> {
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
    if let Some(source_type) = registry.source_type(&type_).filter(|source_type| source_type.info().from_json) {
        source_type.create_from_json(id, value, registry.functions(), error)
    } else {
        error.message = "invalid source type".to_owned();
        None
    }
}
