//! A runtime map's commands for its image sources.

use std::sync::Arc;

use mln_runtime::{Applied, Completion, Map};
use mln_style::source_type::{Source, SourceData};
use mln_util::LatLng;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;

use super::{ImageSourceOptions, ImageSourceType};
use crate::not_a;

/// A runtime map's commands for its image sources. A command for a source
/// of another type completes with
/// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind), and one for a
/// source the style doesn't have with `NotFound`.
pub trait ImageSourceCommands {
    /// Adds an image source that shows `image` at `coordinates`, its four
    /// corners clockwise from the top left.
    fn add_image_source(
        &self,
        source_id: impl Into<String>,
        coordinates: [LatLng; 4],
        image: PremultipliedImage,
    ) -> Result<Completion<Applied>, Error>;

    /// Makes an image source load its image from `url`.
    fn set_image_source_url(
        &self,
        source_id: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<Completion<Applied>, Error>;

    /// Replaces the image of an image source.
    fn set_image_source_image(
        &self,
        source_id: impl Into<String>,
        image: PremultipliedImage,
    ) -> Result<Completion<Applied>, Error>;

    /// Moves the image of an image source to `coordinates`, its four corners
    /// clockwise from the top left.
    fn set_image_source_coordinates(
        &self,
        source_id: impl Into<String>,
        coordinates: [LatLng; 4],
    ) -> Result<Completion<Applied>, Error>;

    /// The corners of an image source's image, clockwise from the top left.
    fn image_source_coordinates(&self, source_id: impl Into<String>) -> Result<Completion<[LatLng; 4]>, Error>;
}

impl ImageSourceCommands for Map {
    fn add_image_source(
        &self,
        source_id: impl Into<String>,
        coordinates: [LatLng; 4],
        image: PremultipliedImage,
    ) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.add_style_source(move |_| {
            let options = ImageSourceOptions { url: None, coordinates };
            let data = SourceData::<ImageSourceType>::with_description(options, Arc::new(image));
            Ok(Source::new(&source_id, data))
        })
    }

    fn set_image_source_url(
        &self,
        source_id: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        self.apply_style_edit(ImageSourceType::set_url(source_id, url))
    }

    fn set_image_source_image(
        &self,
        source_id: impl Into<String>,
        image: PremultipliedImage,
    ) -> Result<Completion<Applied>, Error> {
        self.apply_style_edit(ImageSourceType::set_image(source_id, image))
    }

    fn set_image_source_coordinates(
        &self,
        source_id: impl Into<String>,
        coordinates: [LatLng; 4],
    ) -> Result<Completion<Applied>, Error> {
        self.apply_style_edit(ImageSourceType::set_coordinates(source_id, coordinates))
    }

    fn image_source_coordinates(&self, source_id: impl Into<String>) -> Result<Completion<[LatLng; 4]>, Error> {
        self.query_style_source(source_id, |source| {
            let data =
                SourceData::<ImageSourceType>::recover(source).ok_or_else(|| not_a(source.id(), "an image source"))?;
            Ok(data.options.coordinates)
        })
    }
}
