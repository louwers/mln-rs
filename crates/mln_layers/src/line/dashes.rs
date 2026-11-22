//! The dash patterns of `line-dasharray`, as signed distance fields: the
//! textures of the layer's dash arrays, and the per-bucket atlas of dash
//! arrays that differ by feature. Both find a pattern by the dash array's
//! values themselves (their float bits) and its cap.

use std::rc::Rc;

use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_render::draw::{Resources, Texture};
use mln_util::image::AlphaImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

/// How a mesh samples a texture of dash patterns: linearly, repeating
/// along the line.
pub(crate) const DASH_SAMPLER: SamplerState = SamplerState {
    filter: TextureFilterType::Linear,
    wrap_u: TextureWrapType::Repeat,
    wrap_v: TextureWrapType::Clamp,
    max_anisotropy: 1,
    mipmapped: false,
};

/// Where a dash pattern is in a [`DashTexture`]: its length in dash array
/// units (the texture's width for a solid line), and its middle row and
/// height as fractions of the texture's height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DashPosition {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) y: f32,
}

/// The ends of a pattern's dashes: square, or round for a round
/// `line-cap`, which takes 15 rows rather than one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, mln_wire::Wire)]
pub(crate) enum DashCap {
    #[default]
    Square = 0,
    Round = 1,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DashRange {
    left: f32,
    right: f32,
    is_dash: bool,
    is_zero_length: bool,
}

fn dash_ranges(dasharray: &[f32], stretch: f32) -> Vec<DashRange> {
    // If dasharray has an odd length, both the first and last parts are
    // dashes and should be joined seamlessly.
    let odd_dash_array = dasharray.len() % 2 == 1;

    let mut left = if odd_dash_array { -dasharray[dasharray.len() - 1] * stretch } else { 0.0f32 };
    let mut right = dasharray[0] * stretch;
    let mut is_dash = true;

    let mut ranges = Vec::with_capacity(dasharray.len());
    ranges.push(DashRange { left, right, is_dash, is_zero_length: dasharray[0] == 0.0 });

    let mut current_dash_length = dasharray[0];
    for &dash_length in &dasharray[1..] {
        is_dash = !is_dash;

        left = current_dash_length * stretch;
        current_dash_length += dash_length;
        right = current_dash_length * stretch;

        ranges.push(DashRange { left, right, is_dash, is_zero_length: dash_length == 0.0 });
    }

    ranges
}

fn to_alpha(v: f32) -> u8 {
    mln_math::fmaxf(0.0, mln_math::fminf(255.0, v)) as u8
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
            let min_dist = mln_math::fminf(dist_left, dist_right);

            let dist_middle = y as f32 / n as f32 * (half_stretch + 1.0);
            let signed_distance = if range.is_dash {
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
    ranges.retain(|range| !range.is_zero_length);

    if ranges.is_empty() {
        return;
    }

    if ranges.len() > 1 {
        // Collapse neighbouring same-type parts into a single part
        let mut curr = 0;
        let mut next = 1;
        while next != ranges.len() {
            if ranges[next].is_dash == ranges[curr].is_dash {
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
    if ranges[0].is_dash == ranges[last_index].is_dash {
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
        let min_dist = mln_math::fminf(dist_left, dist_right);
        let signed_distance = if range.is_dash { min_dist } else { -min_dist };

        image.data[(index + x) as usize] = to_alpha(signed_distance + 128.0);
    }
}

fn add_dash_pattern(image: &mut AlphaImage, y_offset: u32, dasharray: &[f32], pattern_cap: DashCap) -> DashPosition {
    let n: u8 = if pattern_cap == DashCap::Round { 7 } else { 0 };
    let image_height = image.size.height as f32;
    let pos = |width| DashPosition {
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

    if pattern_cap == DashCap::Round {
        add_round_dash(&ranges, y_offset, stretch, i32::from(n), image);
    } else {
        add_regular_dash(&mut ranges, y_offset, image);
    }

    pos(length)
}

/// The image of two dash patterns, a `line-dasharray`'s `from` and `to` at
/// the zooms a frame cross-fades between, with `cap` ends, and where each
/// is in it.
fn dash_patterns(from: &[f32], to: &[f32], cap: DashCap) -> (AlphaImage, DashPosition, DashPosition) {
    let patterns_identical = from == to;
    let pattern_height: u32 = if cap == DashCap::Round { 15 } else { 1 };
    let height: u32 = (if patterns_identical { 1 } else { 2 }) * pattern_height;

    // The patterns repeat horizontally, which a texture whose height
    // isn't a power of two can't on OpenGL ES 2 (15 -> 16, 30 -> 32).
    let texture_height = height.next_power_of_two();
    let mut image = AlphaImage::new(Size::new(256, texture_height));

    let from_pos = add_dash_pattern(&mut image, 0, from, cap);
    let to_pos = if patterns_identical { from_pos } else { add_dash_pattern(&mut image, pattern_height, to, cap) };
    (image, from_pos, to_pos)
}

/// A texture of the dash patterns a frame cross-fades between, and where
/// each is in it.
pub(crate) struct DashTexture {
    /// The patterns' dash arrays.
    key: DashKey,
    pub(crate) from: DashPosition,
    pub(crate) to: DashPosition,
    pub(crate) size: Size,
    pub(crate) texture: Texture,
}

/// The textures of a layer's dash array, one for each cap, each holding the
/// patterns of the zooms that the last frame that asked for it
/// cross-faded between.
#[derive(Default)]
pub(crate) struct DashTextures([Option<DashTexture>; 2]);

impl DashTextures {
    /// Makes the texture of `cap` ends hold the patterns `from` and `to`,
    /// unless it does.
    pub(crate) fn set(&mut self, res: &mut Resources, from: &[f32], to: &[f32], cap: DashCap) {
        let slot = &mut self.0[cap as usize];
        if slot.as_ref().is_some_and(|texture| is_dash_key(&texture.key, from, to)) {
            return;
        }
        let (image, from_pos, to_pos) = dash_patterns(from, to, cap);
        let size = image.size;
        let image = Rc::new(image);
        let texture = match slot.take() {
            Some(DashTexture { texture, .. }) => {
                if let Some(mut texture) = res.texture_mut(&texture) {
                    texture.set_alpha_image(image);
                }
                texture
            }
            None => res.texture(|texture| texture.set_alpha_image(image)),
        };
        *slot = Some(DashTexture { key: dash_key(from, to), from: from_pos, to: to_pos, size, texture });
    }

    /// The texture of `cap` ends, once [`set`](Self::set) made it.
    pub(crate) fn get(&self, cap: DashCap) -> Option<&DashTexture> {
        self.0[cap as usize].as_ref()
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
    entries: Vec<(Vec<u32>, DashCap, DashEntry)>,
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
    pub(crate) fn add(&mut self, dasharray: &[f32], cap: DashCap) -> DashEntry {
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
        let rows: u32 = if cap == DashCap::Round { 15 } else { 1 };
        let y_offset = self.next_row;
        let end = y_offset + rows;
        if end > MAX_DASH_ATLAS_HEIGHT {
            Log::warning(Event::Render, "line-dasharray: too many dash patterns in one tile");
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

/// The dash arrays of a texture, as the bits of their values.
type DashKey = (Vec<u32>, Vec<u32>);

fn dash_key(from: &[f32], to: &[f32]) -> DashKey {
    (from.iter().map(|v| v.to_bits()).collect(), to.iter().map(|v| v.to_bits()).collect())
}

/// Whether `key` is that of the dash arrays `from` and `to`.
fn is_dash_key(key: &DashKey, from: &[f32], to: &[f32]) -> bool {
    let same = |bits: &[u32], values: &[f32]| bits.iter().copied().eq(values.iter().map(|v| v.to_bits()));
    same(&key.0, from) && same(&key.1, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dash_array_of_fewer_than_two_parts_or_no_length_is_solid() {
        for dasharray in [&[][..], &[3.0], &[0.0, 0.0]] {
            for (cap, rows) in [(DashCap::Square, 1), (DashCap::Round, 15)] {
                let (image, from, to) = dash_patterns(&[2.0, 1.0], dasharray, cap);
                assert_eq!(from.width, 3.0);
                assert_eq!(to.width, 256.0, "{dasharray:?}");
                let start = (rows * image.size.width) as usize;
                let end = (2 * rows * image.size.width) as usize;
                assert!(image.data[start..end].iter().all(|&alpha| alpha == 255), "{dasharray:?}");
            }
        }
    }
}
