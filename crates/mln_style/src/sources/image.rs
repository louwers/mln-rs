//! Image sources: an image, from the source's URL or the API, drawn between
//! four corner coordinates.

use std::sync::Arc;

use crate::Edit;
use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::geo::LatLng;
use crate::layer::SourceKind;
use crate::source::{Source, SourceData, SourceTypeInfo, StyleSourceType};

/// The image source type.
pub struct ImageSourceType;

/// An image source's options.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageSourceOptions {
    /// The URL of the source's image, if it loads it from one.
    pub url: Option<String>,
    /// The image's corners: top left, top right, bottom right, bottom left.
    pub coordinates: [LatLng; 4],
}

impl StyleSourceType for ImageSourceType {
    const INFO: SourceTypeInfo = SourceTypeInfo { name: "image", layer_kind: SourceKind::Raster, from_json: true };

    type Options = ImageSourceOptions;

    fn from_json(value: &Convertible<'_>, _functions: &Arc<Functions>, error: &mut Error) -> Option<SourceData<Self>> {
        crate::conversion::source::convert_image_source(value, error)
    }
}

impl ImageSourceType {
    /// The edit that makes the image source `id` load its image from `url`.
    /// Its image stays until the URL's arrives.
    pub fn set_url(id: impl Into<String>, url: impl Into<String>) -> Edit {
        let url = url.into();
        Edit::update_source::<Self>(id, "an image source", |data| {
            data.options.url = Some(url);
            data.load += 1;
            true
        })
    }

    /// The edit that moves the corners of the image source `id`: top left,
    /// top right, bottom right, bottom left.
    pub fn set_coordinates(id: impl Into<String>, coordinates: [LatLng; 4]) -> Edit {
        Edit::update_source::<Self>(id, "an image source", move |data| {
            data.options.coordinates = coordinates;
            true
        })
    }
}

/// An image source with its corners at `coordinates` and no image until it
/// gets one or a URL.
#[must_use]
pub fn image_source(id: &str, coordinates: [LatLng; 4]) -> Source {
    Source::new(id, SourceData::<ImageSourceType>::new(ImageSourceOptions { url: None, coordinates }))
}
