//! Image sources: an image, from the source's URL or the API, which the
//! renderer draws between four corner coordinates.

use std::sync::Arc;

use mln_image::decoder;
use mln_render::source::{SourceImage, SourceType, Supply};
use mln_storage::{Data, Resource};
use mln_style::Edit;
use mln_style::conversion::{Convertible, Error, convert};
use mln_style::expression::Functions;
use mln_style::layer_type::SourceKind;
use mln_style::source_type::{
    Described, Load, OfflineResources, Source, SourceData, SourceSummary, SourceTypeInfo, StyleSourceType,
};
use mln_util::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Respond;

#[cfg(feature = "runtime")]
pub use self::commands::ImageSourceCommands;

#[cfg(feature = "runtime")]
mod commands;

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
    // An image source has no tiles, so the tile members say nothing.
    const INFO: SourceTypeInfo = SourceTypeInfo { name: "image", layer_kind: SourceKind::Raster, from_json: true };

    const DESCRIBES_IN_BACKGROUND: bool = true;

    type Options = ImageSourceOptions;
    type Description = Arc<PremultipliedImage>;

    fn from_json(value: &Convertible<'_>, _functions: &Arc<Functions>, error: &mut Error) -> Option<SourceData<Self>> {
        convert_image_source(value, error)
    }

    /// The image at the source's URL, if it has one.
    fn load(source: &SourceData<Self>) -> Load<Arc<PremultipliedImage>> {
        match &source.options.url {
            Some(url) => Load::Fetch(Resource::image(url)),
            None => Load::Done,
        }
    }

    /// Decodes the fetched image, at once or, where the platform decodes
    /// images asynchronously (the browser), later.
    fn describe(
        _options: &ImageSourceOptions,
        data: Option<Data>,
        respond: Respond<Described<Arc<PremultipliedImage>>>,
    ) {
        let Some(data) = data else {
            respond(Err(mln_util::Error::native("unexpectedly empty image url")));
            return;
        };
        decoder::decode(&data, move |result| {
            respond(result.map(|image| Some(Arc::new(image))).map_err(|error| mln_util::Error::native(error.message)));
        });
    }

    fn summary(source: &SourceData<Self>) -> SourceSummary {
        SourceSummary { url: source.options.url.clone(), ..SourceSummary::default() }
    }

    fn offline(source: &SourceData<Self>) -> OfflineResources {
        match &source.options.url {
            Some(url) => OfflineResources::Image { url: url.clone() },
            None => OfflineResources::None,
        }
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

    /// The edit that replaces the image of the image source `id`, which no
    /// longer loads it from a URL.
    pub fn set_image(id: impl Into<String>, image: PremultipliedImage) -> Edit {
        Edit::update_source::<Self>(id, "an image source", |data| {
            data.options.url = None;
            data.description = Some(Arc::new(image));
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

impl SourceType for ImageSourceType {
    fn supply(_source: &Source) -> Supply {
        Supply::image(image_of)
    }
}

/// The image of `source`, if it is an image source.
fn image_of(source: &Source) -> Option<SourceImage<'_>> {
    let data = SourceData::<ImageSourceType>::recover(source)?;
    Some(SourceImage { image: data.description.as_ref(), coordinates: data.options.coordinates })
}

/// An image source's data from its style JSON.
fn convert_image_source(value: &Convertible<'_>, error: &mut Error) -> Option<SourceData<ImageSourceType>> {
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

    let mut coordinates = [LatLng::default(); 4];
    for (i, coordinate) in coordinates.iter_mut().enumerate() {
        *coordinate = convert::<LatLng>(&coordinates_value.array_member(i), error)?;
    }
    Some(SourceData::new(ImageSourceOptions { url: url_string, coordinates }))
}
