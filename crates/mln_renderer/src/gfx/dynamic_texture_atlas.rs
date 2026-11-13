// The atlas is shared by the render thread and the tile workers
// (`DynamicTextureAtlasPtr`, C++ `std::shared_ptr`), so it is `Send + Sync`
// with its state behind a mutex, as in C++. `Context::createDynamicTexture`
// needs no context here: the GPU texture is created in the upload pass (see
// dynamic_texture.rs).

use std::collections::HashMap;
use std::sync::Arc;

use geometry::Point;
use mln_gfx::types::TexturePixelType;
use mln_style_types::image::{ImageMap, ImagePosition, ImagePositions, ImageType, Impl as ImageImpl, RectU16};
use mln_text::glyph::{Glyph, GlyphMap, GlyphPosition, GlyphPositions};
use mln_util::image::{AlphaImage, PremultipliedImage};
use mln_util::serialization::sorted;
use mln_util::size::Size;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::dynamic_texture::{DynamicTexture, DynamicTexturePtr, TextureHandle};
use crate::tile::worker_codec::fields;

pub(crate) type ImageVersionMap = HashMap<String, u32>;

pub(crate) type DynamicTextureAtlasPtr = Arc<DynamicTextureAtlas>;

/// Port-only: what a `LayoutResult`'s atlases release their bins to. In
/// C++ that is always the map's `DynamicTextureAtlas`; for a tile worker in
/// another wasm instance it is the map side's mirror of that instance's
/// atlas (`tile/worker_codec`), which sends the releases on.
pub(crate) trait TextureAtlas: Send + Sync {
    fn remove_textures(&self, texture_handles: &[TextureHandle], dynamic_texture: Option<&DynamicTexturePtr>);
    /// Drops the dynamic textures that are no longer in use. Render thread
    /// only.
    fn remove_unused_dynamic_textures(&self);
}

impl TextureAtlas for DynamicTextureAtlas {
    fn remove_textures(&self, texture_handles: &[TextureHandle], dynamic_texture: Option<&DynamicTexturePtr>) {
        DynamicTextureAtlas::remove_textures(self, texture_handles, dynamic_texture);
    }

    fn remove_unused_dynamic_textures(&self) {
        DynamicTextureAtlas::remove_unused_dynamic_textures(self);
    }
}

const EXTRA_PADDING: u16 = 1;
const PADDING: u16 = ImagePosition::PADDING + EXTRA_PADDING;
const START_SIZE: Size = Size { width: 512, height: 512 };
const DUMMY_SIZE: Size = Size { width: 1, height: 1 };

fn rect_without_extra_padding(rect: RectU16) -> RectU16 {
    RectU16 {
        x: rect.x + EXTRA_PADDING,
        y: rect.y + EXTRA_PADDING,
        w: rect.w - 2 * EXTRA_PADDING,
        h: rect.h - 2 * EXTRA_PADDING,
    }
}

/// Where a tile's glyphs are in the dynamic texture. Serde writes the
/// texture by key, as the worker instance's (`tile/worker_codec`).
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct GlyphAtlas {
    pub(crate) glyph_positions: GlyphPositions,
    pub(crate) texture_handles: Vec<TextureHandle>,
    #[serde(with = "fields::texture")]
    pub(crate) dynamic_texture: Option<DynamicTexturePtr>,
}

/// Where a tile's icons and patterns are in the dynamic texture. Serde
/// writes the texture by key, as the worker instance's (`tile/worker_codec`).
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct ImageAtlas {
    #[serde(with = "sorted::map")]
    pub(crate) icon_positions: ImagePositions,
    #[serde(with = "sorted::map")]
    pub(crate) pattern_positions: ImagePositions,
    pub(crate) texture_handles: Vec<TextureHandle>,
    #[serde(with = "fields::texture")]
    pub(crate) dynamic_texture: Option<DynamicTexturePtr>,
}

/// Identifies an image's allocation: (dynamic texture, image revision,
/// usage).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ImageAllocationKey {
    dynamic_texture: usize,
    /// The revision's address (an identity; the allocation keeps the image
    /// alive).
    image: usize,
    type_: ImageType,
}

struct ImageAllocation {
    // Keeps the revision alive so its address can't be reused by another
    // image while the bin exists.
    _image: Arc<ImageImpl>,
    bin_id: i32,
}

/// Identifies a glyph's allocation: (dynamic texture, glyph). The font stacks
/// that hold the same immutable glyph, which has the same pixels, share its
/// allocation.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphAllocationKey {
    dynamic_texture: usize,
    /// The glyph's address (an identity; the allocation keeps the glyph
    /// alive).
    glyph: usize,
}

struct GlyphAllocation {
    // Keeps the glyph alive so its address can't be reused by another glyph
    // while the bin exists.
    _glyph: Arc<Glyph>,
    bin_id: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct BinKey {
    dynamic_texture: usize,
    bin_id: i32,
}

#[derive(Default)]
struct State {
    dynamic_textures: Vec<DynamicTexturePtr>,
    dummy_dynamic_texture: HashMap<u8, DynamicTexturePtr>,
    image_allocations: HashMap<ImageAllocationKey, ImageAllocation>,
    image_allocation_keys: HashMap<BinKey, ImageAllocationKey>,
    glyph_allocations: HashMap<GlyphAllocationKey, GlyphAllocation>,
    glyph_allocation_keys: HashMap<BinKey, GlyphAllocationKey>,
}

#[derive(Default)]
pub(crate) struct DynamicTextureAtlas {
    state: Mutex<State>,
}

impl Drop for DynamicTextureAtlas {
    /// Frees the GPU textures with the atlas (render thread): textures still
    /// referenced elsewhere must not keep the map's device alive.
    fn drop(&mut self) {
        let state = self.state.get_mut();
        for texture in state.dynamic_textures.iter().chain(state.dummy_dynamic_texture.values()) {
            texture.release_gpu_texture();
        }
    }
}

fn pixel_type_key(t: TexturePixelType) -> u8 {
    t as u8
}

impl DynamicTextureAtlas {
    #[must_use]
    pub(crate) fn new() -> Self {
        DynamicTextureAtlas::default()
    }

    pub(crate) fn upload_glyphs(&self, glyphs: &GlyphMap) -> GlyphAtlas {
        let mut state = self.state.lock();

        let mut glyph_atlas = GlyphAtlas::default();
        if glyphs.is_empty() {
            let texture = Arc::clone(
                state.dummy_dynamic_texture.entry(pixel_type_key(TexturePixelType::Alpha)).or_insert_with(|| {
                    let mut dummy_image = AlphaImage::new(DUMMY_SIZE);
                    dummy_image.fill(0);
                    let texture = Arc::new(DynamicTexture::new(DUMMY_SIZE, TexturePixelType::Alpha));
                    texture.add_image(&dummy_image.data, DUMMY_SIZE, -1);
                    texture
                }),
            );
            glyph_atlas.dynamic_texture = Some(texture);
            return glyph_atlas;
        }

        let mut dyn_tex_index = 0usize;
        let mut dyn_tex_size = START_SIZE;
        let mut glyphs_to_upload: Vec<(TextureHandle, Arc<Glyph>, u64)> = Vec::new();

        while glyph_atlas.dynamic_texture.is_none() {
            let texture = if dyn_tex_index < state.dynamic_textures.len() {
                let t = Arc::clone(&state.dynamic_textures[dyn_tex_index]);
                dyn_tex_index += 1;
                t
            } else {
                let t = Arc::new(DynamicTexture::new(dyn_tex_size, TexturePixelType::Alpha));
                dyn_tex_size = Size::new(dyn_tex_size.width * 2, dyn_tex_size.height * 2);
                dyn_tex_index += 1;
                t
            };

            if texture.get_pixel_format() != TexturePixelType::Alpha {
                continue;
            }
            glyph_atlas.dynamic_texture = Some(Arc::clone(&texture));

            let mut has_space = true;
            for (&font_stack, font_glyphs) in glyphs {
                for glyph in font_glyphs.values() {
                    let Some(glyph) = glyph else { continue };
                    if !glyph.bitmap.valid() {
                        continue;
                    }
                    if let Some(tex_handle) = Self::reserve_glyph(&mut state, &texture, glyph) {
                        glyphs_to_upload.push((tex_handle, Arc::clone(glyph), font_stack));
                    } else {
                        has_space = false;
                        break;
                    }
                }
                if !has_space {
                    for (tex_handle, _, _) in &glyphs_to_upload {
                        Self::release_texture(&mut state, &texture, tex_handle);
                    }
                    glyphs_to_upload.clear();
                    glyph_atlas.dynamic_texture = None;
                    break;
                }
            }
        }
        let texture = glyph_atlas.dynamic_texture.clone().expect("dynamic texture");
        if dyn_tex_index > state.dynamic_textures.len() {
            state.dynamic_textures.push(Arc::clone(&texture));
        }

        for (mut tex_handle, glyph, font_stack) in glyphs_to_upload {
            let rect = *tex_handle.get_rectangle();

            if tex_handle.is_upload_needed() {
                let mut padded_image = AlphaImage::new(Size::new(u32::from(rect.w), u32::from(rect.h)));
                padded_image.fill(0);
                let _ = AlphaImage::copy(
                    &glyph.bitmap,
                    &mut padded_image,
                    Point::new(0, 0),
                    Point::new(u32::from(PADDING), u32::from(PADDING)),
                    glyph.bitmap.size,
                );
                texture.upload_image(&padded_image.data, &mut tex_handle);
            }
            glyph_atlas.texture_handles.push(tex_handle);
            glyph_atlas
                .glyph_positions
                .entry(font_stack)
                .or_default()
                .entry(glyph.id)
                .or_insert(GlyphPosition { rect: rect_without_extra_padding(rect), metrics: glyph.metrics });
        }
        glyph_atlas
    }

    pub(crate) fn upload_icons_and_patterns(
        &self,
        icons: &ImageMap,
        patterns: &ImageMap,
        version_map: &ImageVersionMap,
    ) -> ImageAtlas {
        let mut state = self.state.lock();

        let mut image_atlas = ImageAtlas::default();
        if icons.is_empty() && patterns.is_empty() {
            let texture = Arc::clone(
                state.dummy_dynamic_texture.entry(pixel_type_key(TexturePixelType::RGBA)).or_insert_with(|| {
                    let mut dummy_image = PremultipliedImage::new(DUMMY_SIZE);
                    dummy_image.fill(0);
                    let texture = Arc::new(DynamicTexture::new(DUMMY_SIZE, TexturePixelType::RGBA));
                    texture.add_image(&dummy_image.data, DUMMY_SIZE, -1);
                    texture
                }),
            );
            image_atlas.dynamic_texture = Some(texture);
            return image_atlas;
        }

        let mut dyn_tex_index = 0usize;
        let mut dyn_tex_size = START_SIZE;
        let mut icons_to_upload: Vec<(TextureHandle, Arc<ImageImpl>)> = Vec::with_capacity(icons.len());
        let mut patterns_to_upload: Vec<(TextureHandle, Arc<ImageImpl>)> = Vec::with_capacity(patterns.len());

        while image_atlas.dynamic_texture.is_none() {
            let texture = if dyn_tex_index < state.dynamic_textures.len() {
                let t = Arc::clone(&state.dynamic_textures[dyn_tex_index]);
                dyn_tex_index += 1;
                t
            } else {
                let t = Arc::new(DynamicTexture::new(dyn_tex_size, TexturePixelType::RGBA));
                dyn_tex_size = Size::new(dyn_tex_size.width * 2, dyn_tex_size.height * 2);
                dyn_tex_index += 1;
                t
            };

            if texture.get_pixel_format() != TexturePixelType::RGBA {
                continue;
            }

            let mut has_space = true;
            for icon in icons.values() {
                if let Some(tex_handle) = Self::reserve_image(&mut state, &texture, icon, ImageType::Icon) {
                    icons_to_upload.push((tex_handle, Arc::clone(icon)));
                } else {
                    has_space = false;
                    break;
                }
            }
            if has_space {
                for pattern in patterns.values() {
                    if let Some(tex_handle) = Self::reserve_image(&mut state, &texture, pattern, ImageType::Pattern) {
                        patterns_to_upload.push((tex_handle, Arc::clone(pattern)));
                    } else {
                        has_space = false;
                        break;
                    }
                }
            }
            if !has_space {
                for (tex_handle, _) in &icons_to_upload {
                    Self::release_texture(&mut state, &texture, tex_handle);
                }
                icons_to_upload.clear();
                for (tex_handle, _) in &patterns_to_upload {
                    Self::release_texture(&mut state, &texture, tex_handle);
                }
                patterns_to_upload.clear();
                continue;
            }
            image_atlas.dynamic_texture = Some(texture);
        }
        let texture = image_atlas.dynamic_texture.clone().expect("dynamic texture");
        if dyn_tex_index > state.dynamic_textures.len() {
            state.dynamic_textures.push(Arc::clone(&texture));
        }

        image_atlas.icon_positions.reserve(icons.len());
        for (mut tex_handle, icon) in icons_to_upload {
            let rect = *tex_handle.get_rectangle();

            if tex_handle.is_upload_needed() {
                let mut padded_image = PremultipliedImage::new(Size::new(u32::from(rect.w), u32::from(rect.h)));
                padded_image.fill(0);
                let _ = PremultipliedImage::copy(
                    &icon.image,
                    &mut padded_image,
                    Point::new(0, 0),
                    Point::new(u32::from(PADDING), u32::from(PADDING)),
                    icon.image.size,
                );
                texture.upload_image(&padded_image.data, &mut tex_handle);
            }
            image_atlas.texture_handles.push(tex_handle);
            let version = version_map.get(&icon.id).copied().unwrap_or(0);
            image_atlas
                .icon_positions
                .entry(icon.id.clone())
                .or_insert_with(|| ImagePosition::new(rect_without_extra_padding(rect), &icon, version));
        }

        image_atlas.pattern_positions.reserve(patterns.len());
        for (mut tex_handle, pattern) in patterns_to_upload {
            let rect = *tex_handle.get_rectangle();

            if tex_handle.is_upload_needed() {
                let mut padded_image = PremultipliedImage::new(Size::new(u32::from(rect.w), u32::from(rect.h)));
                padded_image.fill(0);
                let src = &pattern.image;
                let _ = PremultipliedImage::copy(
                    src,
                    &mut padded_image,
                    Point::new(0, 0),
                    Point::new(u32::from(PADDING), u32::from(PADDING)),
                    src.size,
                );

                let x = u32::from(PADDING);
                let y = u32::from(PADDING);
                let w = src.size.width;
                let h = src.size.height;

                // Add 1 pixel wrapped padding on each side of the image.
                let _ = PremultipliedImage::copy(
                    src,
                    &mut padded_image,
                    Point::new(0, h - 1),
                    Point::new(x, y - 1),
                    Size::new(w, 1),
                ); // T
                let _ = PremultipliedImage::copy(
                    src,
                    &mut padded_image,
                    Point::new(0, 0),
                    Point::new(x, y + h),
                    Size::new(w, 1),
                ); // B
                let _ = PremultipliedImage::copy(
                    src,
                    &mut padded_image,
                    Point::new(w - 1, 0),
                    Point::new(x - 1, y),
                    Size::new(1, h),
                ); // L
                let _ = PremultipliedImage::copy(
                    src,
                    &mut padded_image,
                    Point::new(0, 0),
                    Point::new(x + w, y),
                    Size::new(1, h),
                ); // R

                texture.upload_image(&padded_image.data, &mut tex_handle);
            }
            image_atlas.texture_handles.push(tex_handle);
            let version = version_map.get(&pattern.id).copied().unwrap_or(0);
            image_atlas
                .pattern_positions
                .entry(pattern.id.clone())
                .or_insert_with(|| ImagePosition::new(rect_without_extra_padding(rect), &pattern, version));
        }

        image_atlas
    }

    fn reserve_image(
        state: &mut State,
        dynamic_texture: &DynamicTexturePtr,
        image: &Arc<ImageImpl>,
        type_: ImageType,
    ) -> Option<TextureHandle> {
        let size = Size::new(
            image.image.size.width + 2 * u32::from(PADDING),
            image.image.size.height + 2 * u32::from(PADDING),
        );
        let key =
            ImageAllocationKey { dynamic_texture: dynamic_texture.id(), image: Arc::as_ptr(image) as usize, type_ };

        // Reuse the bin of this exact revision; otherwise let the shelf pack
        // assign a fresh id.
        let existing = state.image_allocations.get(&key).map(|a| a.bin_id);
        let tex_handle = dynamic_texture.reserve_size(size, existing.unwrap_or(-1));
        if let Some(tex_handle) = &tex_handle
            && existing.is_none()
        {
            state
                .image_allocations
                .insert(key, ImageAllocation { _image: Arc::clone(image), bin_id: tex_handle.get_id() });
            state
                .image_allocation_keys
                .insert(BinKey { dynamic_texture: dynamic_texture.id(), bin_id: tex_handle.get_id() }, key);
        }
        tex_handle
    }

    fn reserve_glyph(
        state: &mut State,
        dynamic_texture: &DynamicTexturePtr,
        glyph: &Arc<Glyph>,
    ) -> Option<TextureHandle> {
        let size = Size::new(
            glyph.bitmap.size.width + 2 * u32::from(PADDING),
            glyph.bitmap.size.height + 2 * u32::from(PADDING),
        );
        let key = GlyphAllocationKey { dynamic_texture: dynamic_texture.id(), glyph: Arc::as_ptr(glyph) as usize };

        // Reuse the bin of this exact glyph; otherwise let the shelf pack
        // assign a fresh id.
        let existing = state.glyph_allocations.get(&key).map(|a| a.bin_id);
        let tex_handle = dynamic_texture.reserve_size(size, existing.unwrap_or(-1));
        if let Some(tex_handle) = &tex_handle
            && existing.is_none()
        {
            state
                .glyph_allocations
                .insert(key, GlyphAllocation { _glyph: Arc::clone(glyph), bin_id: tex_handle.get_id() });
            state
                .glyph_allocation_keys
                .insert(BinKey { dynamic_texture: dynamic_texture.id(), bin_id: tex_handle.get_id() }, key);
        }
        tex_handle
    }

    fn release_texture(state: &mut State, dynamic_texture: &DynamicTexturePtr, tex_handle: &TextureHandle) {
        if !dynamic_texture.remove_texture(tex_handle) {
            return;
        }

        // The last handle was released, so forget the image revision or
        // glyph; its bin may now be reused.
        let bin_key = BinKey { dynamic_texture: dynamic_texture.id(), bin_id: tex_handle.get_id() };
        if let Some(key) = state.image_allocation_keys.remove(&bin_key) {
            state.image_allocations.remove(&key);
        }
        if let Some(key) = state.glyph_allocation_keys.remove(&bin_key) {
            state.glyph_allocations.remove(&key);
        }
    }

    pub(crate) fn remove_textures(
        &self,
        texture_handles: &[TextureHandle],
        dynamic_texture: Option<&DynamicTexturePtr>,
    ) {
        let Some(dynamic_texture) = dynamic_texture else {
            return;
        };

        let mut state = self.state.lock();

        for tex_handle in texture_handles {
            Self::release_texture(&mut state, dynamic_texture, tex_handle);
        }
    }

    /// Port-only: the textures of the atlas, the dummy ones included, for
    /// the map side's mirror of a tile worker's atlas in another wasm
    /// instance (`tile/worker_codec`).
    pub(crate) fn textures(&self) -> Vec<DynamicTexturePtr> {
        let state = self.state.lock();
        state.dynamic_textures.iter().chain(state.dummy_dynamic_texture.values()).cloned().collect()
    }

    /// Drops the empty dynamic textures that no atlas references any more,
    /// so the last reference goes on the render thread. Render thread only.
    pub(crate) fn remove_unused_dynamic_textures(&self) {
        let mut state = self.state.lock();

        // release textures not referenced elsewhere (by an atlas still alive
        // on a worker thread), so the last reference is always dropped on
        // the render thread
        state.dynamic_textures.retain(|dynamic_texture| {
            let unused = Arc::strong_count(dynamic_texture) == 1 && dynamic_texture.is_empty();
            if unused {
                dynamic_texture.release_gpu_texture();
            }
            !unused
        });
    }
}
