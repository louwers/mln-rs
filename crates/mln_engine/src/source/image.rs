//! The render half of image sources: one image at four coordinates.

use mln_style::Source;
use mln_style::source::SourceData;
use mln_style::sources::ImageSourceType;

use crate::source::{RasterTiles, SourceImage, SourceType, Supply};

impl SourceType for ImageSourceType {
    type Data = RasterTiles;

    fn supply(_source: &Source) -> Supply<RasterTiles> {
        Supply::image(image_of)
    }
}

/// The image of `source`, if it is an image source.
fn image_of(source: &Source) -> Option<SourceImage<'_>> {
    let data = SourceData::<ImageSourceType>::recover(source)?;
    Some(SourceImage { image: data.description.as_ref(), coordinates: data.options.coordinates })
}
