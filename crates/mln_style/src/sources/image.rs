//! Image sources: an image, from the source's URL or the API, drawn between
//! four corner coordinates.

use std::sync::Arc;

use mln_image::decoder;
use mln_storage::{Data, Resource};
use mln_style_types::SourceType;
use mln_util::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Respond;

use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::layer_type::SourceKind;
use crate::source_type::{
    Described, Load, OfflineResources, Source, SourceData, SourceSummary, SourceTypeInfo, StyleSourceType,
};

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
    const INFO: SourceTypeInfo =
        SourceTypeInfo { name: "image", layer_kind: SourceKind::Raster, from_json: true, type_: SourceType::Image };

    const DESCRIBES_IN_BACKGROUND: bool = true;

    type Options = ImageSourceOptions;
    type Description = Arc<PremultipliedImage>;

    fn from_json(value: &Convertible<'_>, _functions: &Arc<Functions>, error: &mut Error) -> Option<SourceData<Self>> {
        crate::conversion::source::convert_image_source(value, error)
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

/// An image source with its corners at `coordinates` and no image until it
/// gets one or a URL.
#[must_use]
pub fn image_source(id: &str, coordinates: [LatLng; 4]) -> Source {
    Source::new(id, SourceData::<ImageSourceType>::new(ImageSourceOptions { url: None, coordinates }))
}
