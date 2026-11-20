//! Image sources: one image, from the source's URL or the API, which the
//! engine draws between the source's four corner coordinates.

use mln_style::Source;
use mln_style::source_type::SourceData;
use mln_style::sources::ImageSourceType;

use crate::source::{SourceImage, SourceType, Supply};

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
