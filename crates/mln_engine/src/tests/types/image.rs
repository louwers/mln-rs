//! The tests' engine half of the image source type: the image at the
//! source's URL or the test's own, which is the source's description.

use std::sync::Arc;

use mln_style::bitmap::PremultipliedImage;
use mln_style::source::SourceData;
use mln_style::sources::ImageSourceType;
use mln_style::sources::image::ImageSourceOptions;
use mln_style::{Edit, Source};

use crate::source::{Described, Fetch, Load, OfflineResources, RasterTiles, SourceImage, SourceType, Supply};

/// The tests' engine half of the image source type.
pub(crate) struct TestImageSource;

impl SourceType for TestImageSource {
    type Style = ImageSourceType;
    type Data = RasterTiles;
    type Description = PremultipliedImage;

    fn load(source: &SourceData<ImageSourceType>) -> Load<PremultipliedImage> {
        match &source.options.url {
            Some(url) => Load::Fetch(Fetch::Image(url.clone())),
            None => Load::Done,
        }
    }

    fn describe_image(_options: &ImageSourceOptions, image: PremultipliedImage) -> Described<PremultipliedImage> {
        Ok(Some(image))
    }

    fn offline(source: &SourceData<ImageSourceType>) -> OfflineResources {
        match &source.options.url {
            Some(url) => OfflineResources::Image { url: url.clone() },
            None => OfflineResources::None,
        }
    }

    fn supply(_source: &Source) -> Supply<RasterTiles> {
        Supply::image(image_of)
    }
}

/// The image of `source`, if it is an image source.
pub(crate) fn image_of(source: &Source) -> Option<SourceImage> {
    let data = SourceData::<ImageSourceType>::recover(source)?;
    let image = data.description.clone().and_then(|image| image.downcast::<PremultipliedImage>().ok());
    Some(SourceImage { image, coordinates: data.options.coordinates })
}

/// The edit that replaces the image of the image source `id`, which no
/// longer loads it from a URL.
pub(crate) fn set_image(id: impl Into<String>, image: PremultipliedImage) -> Edit {
    let image: Arc<PremultipliedImage> = Arc::new(image);
    Edit::update_source::<ImageSourceType>(id, "an image source", |data| {
        data.options.url = None;
        data.description = Some(image);
        data.load += 1;
        true
    })
}
