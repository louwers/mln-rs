// Cut point (added by storage): the sprite loader (mln_sprite) creates
// `style::Image::Impl`s and the style (mln_style) stores them, so the types
// live below both; `mln_style::image` re-exports them.
//
// ImageType, ImageMap, ImageDependencies, ImagePosition and ImagePositions
// (image_impl.hpp) were added by `layers` (M3: buckets and paint property
// binders take ImagePositions).

use std::sync::Arc;

use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::serialization::shared;
use mln_util::size::Size;
use serde::{Deserialize, Serialize};

pub type ImageStretch = (f32, f32);
pub type ImageStretches = Vec<ImageStretch>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextFit {
    StretchOrShrink,
    StretchOnly,
    Proportional,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageContent {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Impl {
    pub id: String,
    pub image: PremultipliedImage,
    /// Pixel ratio of the sprite image.
    pub pixel_ratio: f32,
    /// Whether this image should be interpreted as a signed distance field
    /// icon.
    pub sdf: bool,
    /// Stretch areas of this image.
    pub stretch_x: ImageStretches,
    pub stretch_y: ImageStretches,
    /// The space where text can be fit into this image.
    pub content: Option<ImageContent>,
    /// If `icon-text-fit` is used in a layer with this image, this option
    /// defines constraints on the horizontal scaling of the image.
    pub text_fit_width: Option<TextFit>,
    /// If `icon-text-fit` is used in a layer with this image, this option
    /// defines constraints on the vertical scaling of the image.
    pub text_fit_height: Option<TextFit>,
    /// Whether the image stays in its style once no layer uses it, whatever
    /// the map's policy for unused images says.
    #[serde(default)]
    pub keep_unused: bool,
}

impl PartialEq for Impl {
    /// Image impls compare by id, as in C++.
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

fn validate_stretch(stretches: &ImageStretches, size: f32) -> bool {
    if stretches.is_empty() {
        return true;
    }
    let mut last = 0.0f32;
    for part in stretches {
        if part.0 < last || part.1 < part.0 || size < part.1 {
            return false;
        }
        last = part.1;
    }
    true
}

fn validate_content(content: &ImageContent, size: Size) -> bool {
    let (width, height) = (size.width as f32, size.height as f32);
    if content.left < 0.0 || width < content.left {
        return false;
    }
    if content.top < 0.0 || height < content.top {
        return false;
    }
    if content.right < 0.0 || width < content.right {
        return false;
    }
    if content.bottom < 0.0 || height < content.bottom {
        return false;
    }
    if content.right < content.left {
        return false;
    }
    if content.bottom < content.top {
        return false;
    }
    true
}

impl Impl {
    /// Creates an image. Fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an empty image,
    /// a pixel ratio that isn't positive, or stretches or a content area
    /// outside the image.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        image: PremultipliedImage,
        pixel_ratio: f32,
        sdf: bool,
        stretch_x: ImageStretches,
        stretch_y: ImageStretches,
        content: Option<ImageContent>,
        text_fit_width: Option<TextFit>,
        text_fit_height: Option<TextFit>,
    ) -> Result<Self, Error> {
        if !image.valid() {
            return Err(Error::invalid_argument("dimensions may not be zero"));
        } else if pixel_ratio <= 0.0 {
            return Err(Error::invalid_argument("pixelRatio may not be <= 0"));
        } else if !validate_stretch(&stretch_x, image.size.width as f32) {
            return Err(Error::invalid_argument("stretchX is out of bounds or overlapping"));
        } else if !validate_stretch(&stretch_y, image.size.height as f32) {
            return Err(Error::invalid_argument("stretchY is out of bounds or overlapping"));
        } else if content.as_ref().is_some_and(|content| !validate_content(content, image.size)) {
            return Err(Error::invalid_argument("content area is invalid"));
        }
        Ok(Impl {
            id,
            image,
            pixel_ratio,
            sdf,
            stretch_x,
            stretch_y,
            content,
            text_fit_width,
            text_fit_height,
            keep_unused: false,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Image {
    pub base_impl: Arc<Impl>,
}

impl Image {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        image: PremultipliedImage,
        pixel_ratio: f32,
        sdf: bool,
        stretch_x: ImageStretches,
        stretch_y: ImageStretches,
        content: Option<ImageContent>,
        text_fit_width: Option<TextFit>,
        text_fit_height: Option<TextFit>,
    ) -> Result<Self, Error> {
        Ok(Image {
            base_impl: Arc::new(Impl::new(
                id,
                image,
                pixel_ratio,
                sdf,
                stretch_x,
                stretch_y,
                content,
                text_fit_width,
                text_fit_height,
            )?),
        })
    }

    #[must_use]
    pub fn from_impl(base_impl: Arc<Impl>) -> Self {
        Image { base_impl }
    }

    #[must_use]
    pub fn get_id(&self) -> String {
        self.base_impl.id.clone()
    }

    #[must_use]
    pub fn get_image(&self) -> &PremultipliedImage {
        &self.base_impl.image
    }

    #[must_use]
    pub fn get_pixel_ratio(&self) -> f32 {
        self.base_impl.pixel_ratio
    }

    #[must_use]
    pub fn is_sdf(&self) -> bool {
        self.base_impl.sdf
    }

    #[must_use]
    pub fn get_stretch_x(&self) -> &ImageStretches {
        &self.base_impl.stretch_x
    }

    #[must_use]
    pub fn get_stretch_y(&self) -> &ImageStretches {
        &self.base_impl.stretch_y
    }

    #[must_use]
    pub fn get_content(&self) -> &Option<ImageContent> {
        &self.base_impl.content
    }

    #[must_use]
    pub fn get_text_fit_width(&self) -> &Option<TextFit> {
        &self.base_impl.text_fit_width
    }

    #[must_use]
    pub fn get_text_fit_height(&self) -> &Option<TextFit> {
        &self.base_impl.text_fit_height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ImageType {
    Icon,
    Pattern,
}

/// Images by id.
///
/// A `VecMap` rather than a `HashMap`:
/// `DynamicTextureAtlas::upload_icons_and_patterns` reserves atlas space in
/// iteration order, and a `HashMap`'s order changes from process to process,
/// which would move icons in the atlas and change rendered pixels between runs.
pub type ImageMap = mln_vec_map::VecMap<String, Arc<Impl>>;
pub type ImageDependencies = std::collections::HashMap<String, ImageType>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RectU16 {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// An image's place in an atlas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImagePosition {
    pub padded_rect: RectU16,
    pub pixel_ratio: f32,
    pub sdf: bool,
    pub stretch_x: ImageStretches,
    pub stretch_y: ImageStretches,
    pub content: Option<ImageContent>,
    pub text_fit_width: Option<TextFit>,
    pub text_fit_height: Option<TextFit>,
    pub version: u32,
}

impl ImagePosition {
    pub const PADDING: u16 = 1;

    #[must_use]
    pub fn new(rect: RectU16, image: &Impl, version: u32) -> Self {
        ImagePosition {
            padded_rect: rect,
            pixel_ratio: image.pixel_ratio,
            sdf: image.sdf,
            stretch_x: image.stretch_x.clone(),
            stretch_y: image.stretch_y.clone(),
            content: image.content,
            text_fit_width: image.text_fit_width,
            text_fit_height: image.text_fit_height,
            version,
        }
    }

    /// The top-left corner inside the padding; the sums are computed in `i32`
    /// and narrowed to `u16`, as in C++.
    #[must_use]
    pub fn tl(&self) -> [u16; 2] {
        [
            (i32::from(self.padded_rect.x) + i32::from(Self::PADDING)) as u16,
            (i32::from(self.padded_rect.y) + i32::from(Self::PADDING)) as u16,
        ]
    }

    #[must_use]
    pub fn br(&self) -> [u16; 2] {
        [
            (i32::from(self.padded_rect.x) + i32::from(self.padded_rect.w) - i32::from(Self::PADDING)) as u16,
            (i32::from(self.padded_rect.y) + i32::from(self.padded_rect.h) - i32::from(Self::PADDING)) as u16,
        ]
    }

    #[must_use]
    pub fn tlbr(&self) -> [u16; 4] {
        let tl = self.tl();
        let br = self.br();
        [tl[0], tl[1], br[0], br[1]]
    }

    #[must_use]
    pub fn display_size(&self) -> [f32; 2] {
        [
            (i32::from(self.padded_rect.w) - i32::from(Self::PADDING) * 2) as f32 / self.pixel_ratio,
            (i32::from(self.padded_rect.h) - i32::from(Self::PADDING) * 2) as f32 / self.pixel_ratio,
        ]
    }
}

pub type ImagePositions = std::collections::HashMap<String, ImagePosition>;

/// Port-only: the `shared::Kind` of the images of an `ImageMap`, which cross
/// once per pair of sides, then by key (`mln_util::serialization::shared)`: a
/// tile worker in another wasm instance gets the same icons and patterns for
/// every tile.
pub struct SharedImage;

impl shared::Kind for SharedImage {
    type Value = Impl;
    const NAME: &'static str = "image";

    fn write<S: serde::Serializer>(image: &Impl, serializer: S) -> Result<S::Ok, S::Error> {
        image.serialize(serializer)
    }

    fn read<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Arc<Impl>, D::Error> {
        Impl::deserialize(deserializer).map(Arc::new)
    }
}

/// Port-only: `#[serde(with)]` for an `ImageMap`: its images (keyed by their
/// ids), in id order.
pub mod shared_image_map {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{Arc, ImageMap, Impl, SharedImage, shared};

    #[derive(Serialize, Deserialize)]
    struct Image(
        #[serde(serialize_with = "shared::serialize::<SharedImage, _>")]
        #[serde(deserialize_with = "shared::deserialize::<SharedImage, _>")]
        Arc<Impl>,
    );

    pub fn serialize<S: Serializer>(images: &ImageMap, serializer: S) -> Result<S::Ok, S::Error> {
        let mut images: Vec<_> = images.values().cloned().map(Image).collect();
        images.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        images.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ImageMap, D::Error> {
        Ok(Vec::<Image>::deserialize(deserializer)?.into_iter().map(|image| (image.0.id.clone(), image.0)).collect())
    }
}
