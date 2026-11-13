// The C++ `std::map<size_t, DashPatternTexture>` is keyed by a hash of the
// two dash arrays and the cap ("We're not handling hash collisions here");
// the port keys by the values themselves (the float bits), which is the same
// map without the collisions. The hash is only a cache key.

use mln_gfx::UploadPass;
use mln_gfx::texture2d::{SamplerState, Texture2DPtr};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_util::image::AlphaImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_vec_map::VecMap;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DashRange {
    pub(crate) left: f32,
    pub(crate) right: f32,
    pub(crate) is_dash: bool,
    pub(crate) is_zero_length: bool,
}

fn get_dash_ranges(dasharray: &[f32], stretch: f32) -> Vec<DashRange> {
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
        let min_dist = mln_math::fminf(dist_left, dist_right);
        let signed_distance = if range.is_dash { min_dist } else { -min_dist };

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

    if dasharray.len() < 2 {
        Log::warning(Event::ParseStyle, "line dasharray requires at least two elements");
        return LinePatternPos::default();
    }

    let mut length = 0.0f32;
    for part in dasharray {
        length += part;
    }

    let stretch = image.size.width as f32 / length;
    let mut ranges = get_dash_ranges(dasharray, stretch);

    if pattern_cap == LinePatternCap::Round {
        add_round_dash(&ranges, y_offset, stretch, i32::from(n), image);
    } else {
        add_regular_dash(&mut ranges, y_offset, image);
    }

    LinePatternPos {
        y: (0.5 + y_offset as f32 + f32::from(n)) / image.size.height as f32,
        height: (2.0 * f32::from(n) + 1.0) / image.size.height as f32,
        width: length,
    }
}

enum DashTexture {
    Image(AlphaImage),
    Texture(Texture2DPtr),
}

pub(crate) struct DashPatternTexture {
    from: LinePatternPos,
    to: LinePatternPos,
    texture: DashTexture,
}

impl DashPatternTexture {
    #[must_use]
    pub(crate) fn new(from: &[f32], to: &[f32], cap: LinePatternCap) -> Self {
        let patterns_identical = from == to;
        let pattern_height: u32 = if cap == LinePatternCap::Round { 15 } else { 1 };
        let height: u32 = (if patterns_identical { 1 } else { 2 }) * pattern_height;

        // The OpenGL ES 2.0 spec, section 3.8.2: non-power-of-two textures
        // need CLAMP_TO_EDGE; the dash patterns repeat horizontally, so the
        // texture is a power of two (15 -> 16, 30 -> 32).
        let texture_height = height.next_power_of_two();
        let mut image = AlphaImage::new(Size::new(256, texture_height));

        let from_pos = add_dash_pattern(&mut image, 0, from, cap);
        let to_pos = if patterns_identical { from_pos } else { add_dash_pattern(&mut image, pattern_height, to, cap) };

        DashPatternTexture { from: from_pos, to: to_pos, texture: DashTexture::Image(image) }
    }

    /// Uploads the texture to the GPU to be available when we need it. This
    /// is a lazy operation; the texture is only bound when the data is
    /// uploaded for the first time.
    pub(crate) fn upload(&mut self, upload_pass: &dyn UploadPass) {
        if let DashTexture::Image(image) = &self.texture {
            let temp_texture = upload_pass.get_context().create_texture_2d();
            {
                let mut texture = temp_texture.borrow_mut();
                // Texture2D::upload(const AlphaImage&)
                texture.set_format(TexturePixelType::Alpha, TextureChannelDataType::UnsignedByte);
                let data = if image.data.is_empty() { None } else { Some(image.data.as_slice()) };
                texture.upload_data(data, image.size);
                texture.set_sampler_configuration(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Repeat,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }
            self.texture = DashTexture::Texture(temp_texture);
        }
    }

    /// The uploaded texture, if `upload` ran.
    #[must_use]
    pub(crate) fn get_texture(&self) -> Option<Texture2DPtr> {
        match &self.texture {
            DashTexture::Texture(texture) => Some(texture.clone()),
            DashTexture::Image(_) => None,
        }
    }

    /// Returns the size of the texture image.
    #[must_use]
    pub(crate) fn get_size(&self) -> Size {
        match &self.texture {
            DashTexture::Image(image) => image.size,
            DashTexture::Texture(texture) => texture.borrow().get_size(),
        }
    }

    #[must_use]
    pub(crate) fn get_from(&self) -> &LinePatternPos {
        &self.from
    }

    #[must_use]
    pub(crate) fn get_to(&self) -> &LinePatternPos {
        &self.to
    }
}

type DashKey = (Vec<u32>, Vec<u32>, LinePatternCap);

fn dash_key(from: &[f32], to: &[f32], cap: LinePatternCap) -> DashKey {
    (from.iter().map(|v| v.to_bits()).collect(), to.iter().map(|v| v.to_bits()).collect(), cap)
}

#[derive(Default)]
pub(crate) struct LineAtlas {
    textures: VecMap<DashKey, DashPatternTexture>,
    /// Stores a list of the texture objects that need uploading.
    needs_upload: Vec<DashKey>,
}

impl LineAtlas {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Obtains or creates a texture that has both line patterns in it
    pub(crate) fn get_dash_pattern_texture(
        &mut self,
        from: &[f32],
        to: &[f32],
        cap: LinePatternCap,
    ) -> &mut DashPatternTexture {
        let key = dash_key(from, to, cap);
        if !self.textures.contains_key(&key) {
            self.textures.insert(key.clone(), DashPatternTexture::new(from, to, cap));
            self.needs_upload.push(key.clone());
        }
        self.textures.get_mut(&key).expect("dash pattern texture")
    }

    /// Uploads the textures to the GPU to be available when we need it.
    pub(crate) fn upload(&mut self, upload_pass: &dyn UploadPass) {
        for key in std::mem::take(&mut self.needs_upload) {
            if let Some(texture) = self.textures.get_mut(&key) {
                texture.upload(upload_pass);
            }
        }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }
}
