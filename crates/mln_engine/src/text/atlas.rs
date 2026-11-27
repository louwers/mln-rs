//! Where images and glyphs are in an atlas.

use mln_style::image::{ImageContent, ImageStretches, Impl, TextFit};

/// A rectangle of an atlas, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, mln_wire::Wire)]
pub struct RectU16 {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// An image's place in an atlas.
#[derive(Clone, Debug, PartialEq, mln_wire::Wire)]
pub struct ImagePosition {
    pub padded_rect: RectU16,
    pub pixel_ratio: f32,
    pub sdf: bool,
    pub stretch_x: ImageStretches,
    pub stretch_y: ImageStretches,
    pub content: Option<ImageContent>,
    pub text_fit_width: Option<TextFit>,
    pub text_fit_height: Option<TextFit>,
}

impl ImagePosition {
    /// The texels of padding around an image in its atlas.
    pub const PADDING: u16 = 1;

    /// The position of `image` at `rect` of its atlas, padding included.
    #[must_use]
    pub fn new(rect: RectU16, image: &Impl) -> Self {
        ImagePosition {
            padded_rect: rect,
            pixel_ratio: image.pixel_ratio,
            sdf: image.sdf,
            stretch_x: image.stretch_x.clone(),
            stretch_y: image.stretch_y.clone(),
            content: image.content,
            text_fit_width: image.text_fit_width,
            text_fit_height: image.text_fit_height,
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

    /// The bottom-right corner inside the padding.
    #[must_use]
    pub fn br(&self) -> [u16; 2] {
        [
            (i32::from(self.padded_rect.x) + i32::from(self.padded_rect.w) - i32::from(Self::PADDING)) as u16,
            (i32::from(self.padded_rect.y) + i32::from(self.padded_rect.h) - i32::from(Self::PADDING)) as u16,
        ]
    }

    /// The top-left and the bottom-right corners inside the padding.
    #[must_use]
    pub fn tlbr(&self) -> [u16; 4] {
        let tl = self.tl();
        let br = self.br();
        [tl[0], tl[1], br[0], br[1]]
    }

    /// The image's size in pixels: its texels inside the padding, over its
    /// pixel ratio.
    #[must_use]
    pub fn display_size(&self) -> [f32; 2] {
        [
            (i32::from(self.padded_rect.w) - i32::from(Self::PADDING) * 2) as f32 / self.pixel_ratio,
            (i32::from(self.padded_rect.h) - i32::from(Self::PADDING) * 2) as f32 / self.pixel_ratio,
        ]
    }
}

/// The places of the images of a layout, by id.
pub type ImagePositions = mln_style::hash::HashMap<String, ImagePosition>;
