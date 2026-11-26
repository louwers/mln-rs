use std::sync::Arc;

use geojson::GeoJson;

use super::geojson_options::convert_geojson_options;
use super::{Converter, Convertible, Error};
use crate::expression::Functions;
use crate::source::SourceData;
use crate::sources::geojson::{GeoJSONOptions, GeoJsonSourceOptions, GeoJsonSourceType};
use crate::sources::image::{ImageSourceOptions, ImageSourceType};

/// A GeoJSON source's data from its style JSON: its options, and its data,
/// built at once, or the URL it loads it from.
pub(crate) fn convert_geojson_source(
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
    error: &mut Error,
) -> Option<SourceData<GeoJsonSourceType>> {
    let Some(data_value) = value.object_member("data") else {
        error.message = "GeoJSON source must have a data value".to_owned();
        return None;
    };

    let mut options = GeoJSONOptions::default_options();
    if let Some(converted) = convert_geojson_options(value, functions, error) {
        options = Arc::new(converted);
    }

    let mut options = GeoJsonSourceOptions::new(options);

    if data_value.is_object() {
        let geojson = GeoJson::convert(&data_value, error)?;
        match options.data_builder().build(geojson) {
            Ok(data) => Some(SourceData::with_description(options, data)),
            Err(e) => {
                // A clustered non-point feature fails the style's parse.
                error.message = e.message;
                None
            }
        }
    } else if let Some(url) = data_value.to_string() {
        options.url = Some(url);
        Some(SourceData::new(options))
    } else {
        error.message = "GeoJSON data must be a URL or an object".to_owned();
        None
    }
}

/// An image source's data from its style JSON.
pub(crate) fn convert_image_source(value: &Convertible<'_>, error: &mut Error) -> Option<SourceData<ImageSourceType>> {
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

    let mut coordinates = [crate::geo::LatLng::default(); 4];
    for (i, coordinate) in coordinates.iter_mut().enumerate() {
        let lat_lng = super::convert::<crate::geo::LatLng>(&coordinates_value.array_member(i), error)?;
        *coordinate = lat_lng;
    }
    Some(SourceData::new(ImageSourceOptions { url: url_string, coordinates }))
}
