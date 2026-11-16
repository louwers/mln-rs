//! Image sources: an image, from the source's URL or the API, drawn between
//! four corner coordinates.

use std::sync::Arc;

use mln_style_types::SourceType;
use mln_util::LatLng;
use mln_util::image::PremultipliedImage;

use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::layer_type::SourceKind;
use crate::source_type::{OfflineResources, SourceData, SourceSummary, SourceTypeInfo, StyleSourceType};

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

    type Options = ImageSourceOptions;
    type Description = Arc<PremultipliedImage>;

    fn from_json(
        id: &str,
        value: &Convertible<'_>,
        _functions: &Arc<Functions>,
        error: &mut Error,
    ) -> Option<Box<dyn crate::source::Source>> {
        crate::conversion::source::convert_image_source(id, value, error)
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
