// The line atlas keys its textures by the dash arrays' values themselves
// (their float bits) and the cap.

use mln_gfx::gpu::{Gpu, TextureFormat};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_style::bitmap::AlphaImage;
use mln_style::size::Size;

use crate::draw::context::Context;
use crate::draw::store::SharedTexture;
use crate::draw::texture_size;
use crate::log_target;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LinePatternPos {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum LinePatternCap {
    Square = 0,
    Round = 1,
}

/// The kind of content in a dash range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RangeContent {
    Dash,
    Gap,
    ZeroLengthDash,
    ZeroLengthGap,
}

impl RangeContent {
    fn new(is_dash: bool, is_zero_length: bool) -> Self {
        match (is_dash, is_zero_length) {
            (true, false) => RangeContent::Dash,
            (false, false) => RangeContent::Gap,
            (true, true) => RangeContent::ZeroLengthDash,
            (false, true) => RangeContent::ZeroLengthGap,
        }
    }

    /// Whether this range represents a dash (as opposed to a gap).
    fn is_dash(self) -> bool {
        matches!(self, RangeContent::Dash | RangeContent::ZeroLengthDash)
    }

    /// Whether this range has zero length.
    fn is_zero_length(self) -> bool {
        matches!(self, RangeContent::ZeroLengthDash | RangeContent::ZeroLengthGap)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DashRange {
    pub(crate) left: f32,
    pub(crate) right: f32,
    pub(crate) content: RangeContent,
}

fn dash_ranges(dasharray: &[f32], stretch: f32) -> Vec<DashRange> {
    // If dasharray has an odd length, both the first and last parts are
    // dashes and should be joined seamlessly.
    let odd_dash_array = dasharray.len() % 2 == 1;

    let mut left = if odd_dash_array { -dasharray[dasharray.len() - 1] * stretch } else { 0.0f32 };
    let mut right = dasharray[0] * stretch;
    let mut is_dash = true;

    let mut ranges = Vec::with_capacity(dasharray.len());
    ranges.push(DashRange { left, right, content: RangeContent::new(is_dash, dasharray[0] == 0.0) });

    let mut current_dash_length = dasharray[0];
    for &dash_length in &dasharray[1..] {
        is_dash = !is_dash;

        left = current_dash_length * stretch;
        current_dash_length += dash_length;
        right = current_dash_length * stretch;

        ranges.push(DashRange { left, right, content: RangeContent::new(is_dash, dash_length == 0.0) });
    }

    ranges
}

fn to_alpha(v: f32) -> u8 {
    mln_style::math::fmaxf(0.0, mln_style::math::fminf(255.0, v)) as u8
}

fn add_round_dash(ranges: &[DashRange], y_offset: u32, stretch: f32, n: i32, image: &mut AlphaImage) {
    let half_stretch = stretch * 0.5;

    if ranges.is_empty() {
        return;
    }

    let width = image.size.width;
    for y in -n..=n {
        let row = y_offset as i32 + n + y;
        let index = width.wrapping_mul(row as u32);
        let mut curr_index: u32 = 0;
        let mut range = ranges[curr_index as usize];

        for x in 0..width {
            if range.right == 0.0 {
                if x != 0 {
                    curr_index += 1;
                    if (curr_index as usize) < ranges.len() {
                        range = ranges[curr_index as usize];
                    }
                }
            } else if x as f32 / range.right > 1.0 {
                curr_index += 1;
                if (curr_index as usize) < ranges.len() {
                    range = ranges[curr_index as usize];
                }
            }

            let dist_left = (x as f32 - range.left).abs();
            let dist_right = (x as f32 - range.right).abs();
            let min_dist = mln_style::math::fminf(dist_left, dist_right);

            let dist_middle = y as f32 / n as f32 * (half_stretch + 1.0);
            let signed_distance = if range.content.is_dash() {
                let dist_edge = half_stretch - dist_middle.abs();
                (min_dist * min_dist + dist_edge * dist_edge).sqrt()
            } else {
                half_stretch - (min_dist * min_dist + dist_middle * dist_middle).sqrt()
            };

            image.data[(index + x) as usize] = to_alpha(signed_distance + 128.0);
        }
    }
}

fn add_regular_dash(ranges: &mut Vec<DashRange>, y_offset: u32, image: &mut AlphaImage) {
    // Collapse any zero-length range
    ranges.retain(|range| !range.content.is_zero_length());

    if ranges.is_empty() {
        return;
    }

    if ranges.len() > 1 {
        // Collapse neighbouring same-type parts into a single part
        let mut curr = 0;
        let mut next = 1;
        while next != ranges.len() {
            if ranges[next].content.is_dash() == ranges[curr].content.is_dash() {
                ranges[next].left = ranges[curr].left;
                ranges.remove(curr);
            } else {
                curr += 1;
            }
            next = curr + 1;
        }
    }

    let width = image.size.width;
    let last_index = ranges.len() - 1;
    if ranges[0].content.is_dash() == ranges[last_index].content.is_dash() {
        // `last.left - image.size.width` (float - uint32_t: float)
        ranges[0].left = ranges[last_index].left - width as f32;
        ranges[last_index].right = ranges[0].right + width as f32;
    }

    let index = width.wrapping_mul(y_offset);
    let mut curr_index: u32 = 0;
    let mut range = ranges[curr_index as usize];

    for x in 0..width {
        if x as f32 / range.right > 1.0 {
            curr_index += 1;
            if (curr_index as usize) < ranges.len() {
                range = ranges[curr_index as usize];
            }
        }

        let dist_left = (x as f32 - range.left).abs();
        let dist_right = (x as f32 - range.right).abs();
        let min_dist = mln_style::math::fminf(dist_left, dist_right);
        let signed_distance = if range.content.is_dash() { min_dist } else { -min_dist };

        image.data[(index + x) as usize] = to_alpha(signed_distance + 128.0);
    }
}

fn add_dash_pattern(
    image: &mut AlphaImage,
    y_offset: u32,
    dasharray: &[f32],
    pattern_cap: LinePatternCap,
) -> LinePatternPos {
    let n: u8 = if pattern_cap == LinePatternCap::Round { 7 } else { 0 };
    let image_height = image.size.height as f32;
    let pos = |width| LinePatternPos {
        y: (0.5 + y_offset as f32 + f32::from(n)) / image_height,
        height: (2.0 * f32::from(n) + 1.0) / image_height,
        width,
    };

    let mut length = 0.0f32;
    for part in dasharray {
        length += part;
    }

    // As in a `DashAtlas`, a dash array with fewer than two parts or no
    // length is solid. The width is the image's, as the draw's uniforms
    // divide by it and take the smaller of a cross-fade's two.
    if dasharray.len() < 2 || length.is_nan() || length <= 0.0 {
        let width = image.size.width;
        let rows = (y_offset * width) as usize..((y_offset + 2 * u32::from(n) + 1) * width) as usize;
        image.data[rows].fill(255);
        return pos(width as f32);
    }

    let stretch = image.size.width as f32 / length;
    let mut ranges = dash_ranges(dasharray, stretch);

    if pattern_cap == LinePatternCap::Round {
        add_round_dash(&ranges, y_offset, stretch, i32::from(n), image);
    } else {
        add_regular_dash(&mut ranges, y_offset, image);
    }

    pos(length)
}

/// A texture of two dash patterns, a `line-dasharray`'s at the zooms a
/// frame cross-fades between, and where each is in it.
pub(crate) struct DashPatternTexture {
    from: LinePatternPos,
    to: LinePatternPos,
    size: Size,
    /// The patterns, until the texture uploads them.
    image: Option<AlphaImage>,
    texture: Option<SharedTexture>,
}

impl DashPatternTexture {
    #[must_use]
    pub(crate) fn new(from: &[f32], to: &[f32], cap: LinePatternCap) -> Self {
        let patterns_identical = from == to;
        let pattern_height: u32 = if cap == LinePatternCap::Round { 15 } else { 1 };
        let height: u32 = (if patterns_identical { 1 } else { 2 }) * pattern_height;

        // The patterns repeat horizontally, which a texture whose height
        // isn't a power of two can't on OpenGL ES 2 (15 -> 16, 30 -> 32).
        let texture_height = height.next_power_of_two();
        let mut image = AlphaImage::new(Size::new(256, texture_height));

        let from_pos = add_dash_pattern(&mut image, 0, from, cap);
        let to_pos = if patterns_identical { from_pos } else { add_dash_pattern(&mut image, pattern_height, to, cap) };

        DashPatternTexture { from: from_pos, to: to_pos, size: image.size, image: Some(image), texture: None }
    }

    /// Makes the texture and uploads the patterns to it, unless it did.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        let Some(image) = self.image.take() else {
            return;
        };
        let texture = context.add_texture_with(|texture| {
            texture.set_format(TextureFormat::A8Unorm);
            texture.set_sampler(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Repeat,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        });
        if let Err(error) = context.upload_texture_data(gpu, &texture, &image.data, texture_size(image.size)) {
            context.fail(format!("Uploading a dash pattern failed: {error}"));
        }
        self.texture = Some(texture);
    }

    /// The texture, once `upload` made it.
    #[must_use]
    pub(crate) fn texture(&self) -> Option<&SharedTexture> {
        self.texture.as_ref()
    }

    /// The size of the texture.
    #[must_use]
    pub(crate) fn size(&self) -> Size {
        self.size
    }

    /// Where the pattern of the zoom the frame fades from is.
    #[must_use]
    pub(crate) fn from(&self) -> &LinePatternPos {
        &self.from
    }

    /// Where the pattern of the zoom the frame fades to is.
    #[must_use]
    pub(crate) fn to(&self) -> &LinePatternPos {
        &self.to
    }
}

/// Where a dash pattern is in a [`DashAtlas`], in texels: the row at its
/// middle, the distance between its first and last rows, and its length in
/// dash array units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DashEntry {
    pub(crate) y: f32,
    pub(crate) height: f32,
    pub(crate) width: f32,
}

/// The dash patterns of one bucket's features, for a `line-dasharray` that
/// differs by feature: a texture 256 texels wide with a pattern every row,
/// or every 15 rows for round caps, and where each pattern is. The first row
/// is solid. The texture's height is a power of two, as the patterns repeat
/// horizontally.
#[derive(Clone, Debug, Default, PartialEq, mln_wire::Wire)]
pub(crate) struct DashAtlas {
    image: AlphaImage,
    #[wire(skip)]
    next_row: u32,
    /// The patterns added so far, by dash array (as float bits) and cap.
    #[wire(skip)]
    entries: Vec<(Vec<u32>, LinePatternCap, DashEntry)>,
}

/// The tallest a [`DashAtlas`] grows; WebGPU guarantees 8192.
const MAX_DASH_ATLAS_HEIGHT: u32 = 4096;
const DASH_ATLAS_WIDTH: u32 = 256;

/// The solid pattern. Its width is large so that its edges stay sharp.
const SOLID_DASH: DashEntry = DashEntry { y: 0.0, height: 0.0, width: DASH_ATLAS_WIDTH as f32 };

impl DashAtlas {
    /// The pattern of `dasharray` with `cap` ends, added if it isn't yet. A
    /// dash array with fewer than two parts or no length is solid, as is
    /// every pattern once the atlas is full.
    pub(crate) fn add(&mut self, dasharray: &[f32], cap: LinePatternCap) -> DashEntry {
        if self.next_row == 0 {
            self.image = AlphaImage::from_vec(Size::new(DASH_ATLAS_WIDTH, 1), vec![255; DASH_ATLAS_WIDTH as usize]);
            self.next_row = 1;
        }
        let length: f32 = dasharray.iter().sum();
        if dasharray.len() < 2 || length.is_nan() || length <= 0.0 {
            return SOLID_DASH;
        }
        let key: Vec<u32> = dasharray.iter().map(|v| v.to_bits()).collect();
        if let Some((_, _, entry)) = self.entries.iter().find(|(k, c, _)| *k == key && *c == cap) {
            return *entry;
        }
        let rows: u32 = if cap == LinePatternCap::Round { 15 } else { 1 };
        let y_offset = self.next_row;
        let end = y_offset + rows;
        if end > MAX_DASH_ATLAS_HEIGHT {
            log::warn!(target: log_target::RENDER, "line-dasharray: too many dash patterns in one tile");
            return SOLID_DASH;
        }
        if end > self.image.size.height {
            self.image.resize(Size::new(DASH_ATLAS_WIDTH, end.next_power_of_two()));
        }
        let mut pattern = AlphaImage::new(Size::new(DASH_ATLAS_WIDTH, rows));
        let pos = add_dash_pattern(&mut pattern, 0, dasharray, cap);
        let start = (y_offset * DASH_ATLAS_WIDTH) as usize;
        self.image.data[start..start + pattern.data.len()].copy_from_slice(&pattern.data);
        self.next_row = end;

        // The height spans from the first row's middle to the last's, so
        // that sampling stays within the pattern's rows.
        let entry = DashEntry { y: (y_offset + rows / 2) as f32, height: (rows - 1) as f32, width: pos.width };
        self.entries.push((key, cap, entry));
        entry
    }

    /// The atlas, empty when no feature has been added.
    #[must_use]
    pub(crate) fn image(&self) -> &AlphaImage {
        &self.image
    }
}

/// The dash arrays of a texture, as the bits of their values, and its cap.
type DashKey = (Vec<u32>, Vec<u32>, LinePatternCap);

fn dash_key(from: &[f32], to: &[f32], cap: LinePatternCap) -> DashKey {
    (from.iter().map(|v| v.to_bits()).collect(), to.iter().map(|v| v.to_bits()).collect(), cap)
}

/// Whether `key` is that of the dash arrays `from` and `to` with `cap`.
fn is_dash_key(key: &DashKey, from: &[f32], to: &[f32], cap: LinePatternCap) -> bool {
    let same = |bits: &[u32], values: &[f32]| bits.iter().copied().eq(values.iter().map(|v| v.to_bits()));
    key.2 == cap && same(&key.0, from) && same(&key.1, to)
}

#[derive(Default)]
pub(crate) struct LineAtlas {
    /// The textures, in the order they were made, with their keys.
    textures: Vec<(DashKey, DashPatternTexture)>,
    /// The indexes of the textures that need uploading.
    needs_upload: Vec<usize>,
}

impl LineAtlas {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The texture of the dash patterns `from` and `to` with `cap` ends,
    /// made if the atlas has none, which the next upload uploads.
    pub(crate) fn add(&mut self, from: &[f32], to: &[f32], cap: LinePatternCap) -> &DashPatternTexture {
        let index = self.textures.iter().position(|(key, _)| is_dash_key(key, from, to, cap)).unwrap_or_else(|| {
            self.textures.push((dash_key(from, to, cap), DashPatternTexture::new(from, to, cap)));
            self.needs_upload.push(self.textures.len() - 1);
            self.textures.len() - 1
        });
        &self.textures[index].1
    }

    /// The texture of the dash patterns `from` and `to` with `cap` ends, if
    /// the atlas has it.
    #[must_use]
    pub(crate) fn get(&self, from: &[f32], to: &[f32], cap: LinePatternCap) -> Option<&DashPatternTexture> {
        self.textures.iter().find(|(key, _)| is_dash_key(key, from, to, cap)).map(|(_, texture)| texture)
    }

    /// Uploads the textures to the GPU to be available when we need it.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        for index in self.needs_upload.drain(..) {
            self.textures[index].1.upload(gpu, context);
        }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dash_array_of_fewer_than_two_parts_or_no_length_is_solid() {
        for dasharray in [&[][..], &[3.0], &[0.0, 0.0]] {
            for (cap, rows) in [(LinePatternCap::Square, 1), (LinePatternCap::Round, 15)] {
                let texture = DashPatternTexture::new(&[2.0, 1.0], dasharray, cap);
                assert_eq!(texture.from().width, 3.0);
                assert_eq!(texture.to().width, 256.0, "{dasharray:?}");
                let image = texture.image.as_ref().unwrap();
                let start = (rows * image.size.width) as usize;
                let end = (2 * rows * image.size.width) as usize;
                assert!(image.data[start..end].iter().all(|&alpha| alpha == 255), "{dasharray:?}");
            }
        }
    }
}
