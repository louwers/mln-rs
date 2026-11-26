// The atlas of the glyphs and images the tiles' layouts draw, owned by the
// render thread. A tile's dependencies are packed into one glyph page and one
// image page; the tile holds `AtlasLease`s on the bins it was given until
// its layout goes, and dropping a lease queues its bins for the atlas to free
// the next time it packs or uploads.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use geometry::Point;
use mln_gfx::gpu::Gpu;
use mln_gfx::types::TexturePixelType;
use mln_style::bitmap::PremultipliedImage;
use mln_style::hash::{HashMap, RandomState};
use mln_style::image::Impl as ImageImpl;
use mln_style::size::Size;

#[cfg(test)]
use super::dynamic_texture::QueuedPixels;
use super::dynamic_texture::{DynamicTexture, DynamicTextureBinId, TextureHandle};
use crate::draw::context::Context;
use crate::draw::store::SharedTexture;
use crate::image::image_store::{ImageMap, ImageType};
use crate::text::atlas::{ImagePosition, ImagePositions, RectU16};
use crate::text::glyph::{Glyph, GlyphID, GlyphMap, GlyphPosition, GlyphPositions};

const EXTRA_PADDING: u16 = 1;
const PADDING: u16 = ImagePosition::PADDING + EXTRA_PADDING;
const START_SIZE: Size = Size { width: 512, height: 512 };
const DUMMY_SIZE: Size = Size { width: 1, height: 1 };
/// The bin of a 1×1 page: the first of its page.
const DUMMY_BIN: DynamicTextureBinId = DynamicTextureBinId::first();

fn rect_without_extra_padding(rect: RectU16) -> RectU16 {
    RectU16 {
        x: rect.x + EXTRA_PADDING,
        y: rect.y + EXTRA_PADDING,
        w: rect.w - 2 * EXTRA_PADDING,
        h: rect.h - 2 * EXTRA_PADDING,
    }
}

/// `image` placed in a bin of `bin`'s size. A pattern also gets a pixel of
/// its opposite edge on each side, so that it tiles without seams.
fn padded_image(image: &PremultipliedImage, bin: RectU16, type_: ImageType) -> PremultipliedImage {
    let mut padded = PremultipliedImage::new(Size::new(u32::from(bin.w), u32::from(bin.h)));
    padded.fill(0);
    let x = u32::from(PADDING);
    let y = u32::from(PADDING);
    let _ = PremultipliedImage::copy(image, &mut padded, Point::new(0, 0), Point::new(x, y), image.size);

    if type_ == ImageType::Pattern {
        let w = image.size.width;
        let h = image.size.height;
        let _ =
            PremultipliedImage::copy(image, &mut padded, Point::new(0, h - 1), Point::new(x, y - 1), Size::new(w, 1)); // T
        let _ = PremultipliedImage::copy(image, &mut padded, Point::new(0, 0), Point::new(x, y + h), Size::new(w, 1)); // B
        let _ =
            PremultipliedImage::copy(image, &mut padded, Point::new(w - 1, 0), Point::new(x - 1, y), Size::new(1, h)); // L
        let _ = PremultipliedImage::copy(image, &mut padded, Point::new(0, 0), Point::new(x + w, y), Size::new(1, h)); // R
    }
    padded
}

/// A page of the atlas, as the atlas numbers it. Never reused within an
/// atlas.
pub(crate) type PageId = u64;

/// Bins a lease gives back: the page's id and the handles.
type Release = (PageId, Vec<TextureHandle>);

/// What a lease's bin is for, so that a lease can be cut down to some of
/// its bins.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BinUse {
    /// A glyph of a font stack.
    Glyph(u64, GlyphID),
    /// An image, as an icon or a pattern.
    Image(String, ImageType),
    /// The bin of a 1×1 page, for a layout with nothing on its page.
    Dummy,
}

/// A hold on bins of one atlas page, for a layout that draws from them.
/// Dropping it gives the bins back; the atlas frees them the next time it
/// packs or uploads.
pub(crate) struct AtlasLease {
    page: PageId,
    texture_handles: Vec<TextureHandle>,
    /// What each of `texture_handles` is for.
    uses: Vec<BinUse>,
    releases: Sender<Release>,
}

impl AtlasLease {
    /// The handles of the bins the lease holds, one per use.
    #[cfg(test)]
    pub(crate) fn texture_handles(&self) -> &[TextureHandle] {
        &self.texture_handles
    }

    /// The id of the page the bins are on.
    pub(crate) fn page(&self) -> PageId {
        self.page
    }
}

impl Drop for AtlasLease {
    fn drop(&mut self) {
        if !self.texture_handles.is_empty() {
            // The atlas may be gone, and its bins with it.
            let _ = self.releases.send((self.page, std::mem::take(&mut self.texture_handles)));
        }
    }
}

/// Where a tile's glyphs are, on one page.
pub(crate) struct GlyphPage {
    pub(crate) positions: GlyphPositions,
    pub(crate) lease: AtlasLease,
}

/// Where a tile's icons and patterns are, on one page.
pub(crate) struct ImagePage {
    pub(crate) icons: ImagePositions,
    pub(crate) patterns: ImagePositions,
    pub(crate) lease: AtlasLease,
}

/// Identifies an image's allocation: (page, image revision, usage).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ImageAllocationKey {
    page: PageId,
    /// The revision's address (an identity; the allocation keeps the image
    /// alive).
    image: usize,
    type_: ImageType,
}

struct ImageAllocation {
    // Keeps the revision alive so its address can't be reused by another
    // image while the bin exists, and gives a same-size update its id and
    // size.
    image: Arc<ImageImpl>,
    bin_id: DynamicTextureBinId,
}

/// Identifies a glyph's allocation: (page, glyph). The font stacks that hold
/// the same immutable glyph, which has the same pixels, share its allocation.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphAllocationKey {
    page: PageId,
    /// The glyph's address (an identity; the allocation keeps the glyph
    /// alive).
    glyph: usize,
}

struct GlyphAllocation {
    // Keeps the glyph alive so its address can't be reused by another glyph
    // while the bin exists.
    _glyph: Arc<Glyph>,
    bin_id: DynamicTextureBinId,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct BinKey {
    page: PageId,
    bin_id: DynamicTextureBinId,
}

struct Page {
    id: PageId,
    texture: DynamicTexture,
}

/// The pages of the glyphs and images that layouts draw, with their bins
/// shared and refcounted by glyph or image revision.
pub(crate) struct DynamicTextureAtlas {
    pages: Vec<Page>,
    /// A 1×1 page for each pixel type, for layouts with nothing on a page,
    /// while one of them holds it.
    dummies: Vec<Page>,
    next_page: PageId,
    image_allocations: HashMap<ImageAllocationKey, ImageAllocation>,
    image_allocation_keys: HashMap<BinKey, ImageAllocationKey>,
    glyph_allocations: HashMap<GlyphAllocationKey, GlyphAllocation>,
    glyph_allocation_keys: HashMap<BinKey, GlyphAllocationKey>,
    releases: Sender<Release>,
    released: Receiver<Release>,
}

impl Default for DynamicTextureAtlas {
    fn default() -> Self {
        let (releases, released) = channel();
        DynamicTextureAtlas {
            pages: Vec::new(),
            dummies: Vec::new(),
            next_page: 0,
            image_allocations: HashMap::default(),
            image_allocation_keys: HashMap::default(),
            glyph_allocations: HashMap::default(),
            glyph_allocation_keys: HashMap::default(),
            releases,
            released,
        }
    }
}

impl DynamicTextureAtlas {
    #[must_use]
    pub(crate) fn new() -> Self {
        DynamicTextureAtlas::default()
    }

    fn lease(&self, page: &Page, texture_handles: Vec<TextureHandle>, uses: Vec<BinUse>) -> AtlasLease {
        debug_assert_eq!(texture_handles.len(), uses.len());
        AtlasLease { page: page.id, texture_handles, uses, releases: self.releases.clone() }
    }

    /// A lease on the bins of `lease` that `keep` keeps, on the same page.
    /// `lease` keeps its own.
    pub(crate) fn retain(&mut self, lease: &AtlasLease, mut keep: impl FnMut(&BinUse) -> bool) -> AtlasLease {
        let mut texture_handles = Vec::new();
        let mut uses = Vec::new();
        if let Some(page) = self.pages.iter_mut().chain(&mut self.dummies).find(|page| page.id == lease.page) {
            for (handle, bin_use) in lease.texture_handles.iter().zip(&lease.uses) {
                if !keep(bin_use) {
                    continue;
                }
                // `lease` holds the bin, so its id still names it.
                let rect = handle.rectangle();
                let size = Size::new(u32::from(rect.w), u32::from(rect.h));
                if let Some(handle) = page.texture.reserve_size(size, Some(handle.id())) {
                    texture_handles.push(handle);
                    uses.push(bin_use.clone());
                }
            }
        }
        AtlasLease { page: lease.page, texture_handles, uses, releases: self.releases.clone() }
    }

    /// A lease on the bin of the 1×1 page of `pixel_type`.
    fn dummy_lease(&mut self, pixel_type: TexturePixelType) -> AtlasLease {
        let existing = self.dummies.iter().position(|page| page.texture.pixel_format() == pixel_type);
        let index = if let Some(index) = existing {
            index
        } else {
            let id = self.new_page_id();
            self.dummies.push(Page { id, texture: DynamicTexture::new(DUMMY_SIZE, pixel_type) });
            self.dummies.len() - 1
        };
        let texture = &mut self.dummies[index].texture;
        let handle = if texture.is_empty() {
            let bytes = DUMMY_SIZE.width as usize * if pixel_type == TexturePixelType::Alpha { 1 } else { 4 };
            texture.add_image(vec![0; bytes], DUMMY_SIZE, None)
        } else {
            texture.reserve_size(DUMMY_SIZE, Some(DUMMY_BIN))
        };
        let texture_handles: Vec<TextureHandle> = handle.into_iter().collect();
        let uses = vec![BinUse::Dummy; texture_handles.len()];
        self.lease(&self.dummies[index], texture_handles, uses)
    }

    fn new_page_id(&mut self) -> PageId {
        self.next_page += 1;
        self.next_page
    }

    /// Packs `glyphs` into one page. A glyph that is `None` is `None` in the
    /// positions too, and one without a bitmap has an empty rectangle.
    pub(crate) fn pack_glyphs(&mut self, glyphs: &GlyphMap) -> GlyphPage {
        self.free_released();
        let drawn: Vec<(BinUse, &Arc<Glyph>)> = glyphs
            .iter()
            .flat_map(|(font_stack, glyphs)| {
                glyphs.iter().filter_map(|(id, glyph)| {
                    glyph
                        .as_ref()
                        .filter(|glyph| glyph.bitmap.valid())
                        .map(|glyph| (BinUse::Glyph(*font_stack, *id), glyph))
                })
            })
            .collect();
        let (lease, texture_handles) = if drawn.is_empty() {
            (self.dummy_lease(TexturePixelType::Alpha), Vec::new())
        } else {
            let (index, mut texture_handles) = self.place(TexturePixelType::Alpha, |atlas, page, handles| {
                for (_, glyph) in &drawn {
                    let Some(handle) = atlas.reserve_glyph(page, glyph) else {
                        return false;
                    };
                    handles.push(handle);
                }
                true
            });
            let texture = &mut self.pages[index].texture;
            for (tex_handle, (_, glyph)) in texture_handles.iter_mut().zip(&drawn) {
                if tex_handle.is_upload_needed() {
                    texture.upload_glyph(Arc::clone(glyph), PADDING, tex_handle);
                }
            }
            let rects: Vec<RectU16> = texture_handles.iter().map(|handle| *handle.rectangle()).collect();
            let uses = drawn.into_iter().map(|(bin_use, _)| bin_use).collect();
            (self.lease(&self.pages[index], texture_handles, uses), rects)
        };

        let mut rects = texture_handles.into_iter();
        let mut positions = GlyphPositions::new();
        for (&font_stack, font_glyphs) in glyphs {
            let font_positions = positions.entry(font_stack).or_default();
            for (&id, glyph) in font_glyphs {
                let position = glyph.as_ref().map(|glyph| GlyphPosition {
                    rect: if glyph.bitmap.valid() {
                        rects.next().map_or_else(RectU16::default, rect_without_extra_padding)
                    } else {
                        RectU16::default()
                    },
                    metrics: glyph.metrics,
                });
                font_positions.insert(id, position);
            }
        }
        GlyphPage { positions, lease }
    }

    /// Packs `icons` and `patterns` into one page.
    pub(crate) fn pack_images(&mut self, icons: &ImageMap, patterns: &ImageMap) -> ImagePage {
        self.free_released();
        let mut positions = (
            ImagePositions::with_capacity_and_hasher(icons.len(), RandomState::default()),
            ImagePositions::with_capacity_and_hasher(patterns.len(), RandomState::default()),
        );
        if icons.is_empty() && patterns.is_empty() {
            let lease = self.dummy_lease(TexturePixelType::RGBA);
            return ImagePage { icons: positions.0, patterns: positions.1, lease };
        }
        let images: Vec<(&Arc<ImageImpl>, ImageType)> = icons
            .values()
            .map(|icon| (icon, ImageType::Icon))
            .chain(patterns.values().map(|pattern| (pattern, ImageType::Pattern)))
            .collect();
        let (index, mut texture_handles) = self.place(TexturePixelType::RGBA, |atlas, page, handles| {
            for (image, type_) in &images {
                let Some(handle) = atlas.reserve_image(page, image, *type_) else {
                    return false;
                };
                handles.push(handle);
            }
            true
        });

        let texture = &mut self.pages[index].texture;
        for (tex_handle, (image, type_)) in texture_handles.iter_mut().zip(&images) {
            let rect = *tex_handle.rectangle();
            if tex_handle.is_upload_needed() {
                texture.upload_image(padded_image(&image.image, rect, *type_).data, tex_handle);
            }
            let positions = match type_ {
                ImageType::Icon => &mut positions.0,
                ImageType::Pattern => &mut positions.1,
            };
            positions
                .entry(image.id.clone())
                .or_insert_with(|| ImagePosition::new(rect_without_extra_padding(rect), image));
        }
        let uses = images.iter().map(|(image, type_)| BinUse::Image(image.id.clone(), *type_)).collect();
        let lease = self.lease(&self.pages[index], texture_handles, uses);
        ImagePage { icons: positions.0, patterns: positions.1, lease }
    }

    /// The index of the first page of `pixel_type` that `reserve` fits on,
    /// in a new page if none does, and the handles it reserved there. A
    /// page that `reserve` doesn't fit on gets back what it reserved.
    fn place(
        &mut self,
        pixel_type: TexturePixelType,
        mut reserve: impl FnMut(&mut Self, usize, &mut Vec<TextureHandle>) -> bool,
    ) -> (usize, Vec<TextureHandle>) {
        let mut handles = Vec::new();
        for index in 0..self.pages.len() {
            if self.pages[index].texture.pixel_format() != pixel_type {
                continue;
            }
            if reserve(self, index, &mut handles) {
                return (index, handles);
            }
            self.release_handles(index, &handles);
            handles.clear();
        }
        let mut size = START_SIZE;
        loop {
            let id = self.new_page_id();
            self.pages.push(Page { id, texture: DynamicTexture::new(size, pixel_type) });
            let index = self.pages.len() - 1;
            if reserve(self, index, &mut handles) {
                return (index, handles);
            }
            self.release_handles(index, &handles);
            handles.clear();
            self.pages.pop();
            size = Size::new(size.width * 2, size.height * 2);
        }
    }

    fn reserve_image(&mut self, index: usize, image: &Arc<ImageImpl>, type_: ImageType) -> Option<TextureHandle> {
        let page = &mut self.pages[index];
        let size = Size::new(
            image.image.size.width + 2 * u32::from(PADDING),
            image.image.size.height + 2 * u32::from(PADDING),
        );
        let key = ImageAllocationKey { page: page.id, image: Arc::as_ptr(image) as usize, type_ };

        // Reuse the bin of this exact revision; otherwise let the shelf pack
        // assign a fresh id.
        let existing = self.image_allocations.get(&key).map(|a| a.bin_id);
        let tex_handle = page.texture.reserve_size(size, existing);
        if let Some(tex_handle) = &tex_handle
            && existing.is_none()
        {
            self.image_allocations.insert(key, ImageAllocation { image: Arc::clone(image), bin_id: tex_handle.id() });
            self.image_allocation_keys.insert(BinKey { page: page.id, bin_id: tex_handle.id() }, key);
        }
        tex_handle
    }

    fn reserve_glyph(&mut self, index: usize, glyph: &Arc<Glyph>) -> Option<TextureHandle> {
        let page = &mut self.pages[index];
        let size = Size::new(
            glyph.bitmap.size.width + 2 * u32::from(PADDING),
            glyph.bitmap.size.height + 2 * u32::from(PADDING),
        );
        let key = GlyphAllocationKey { page: page.id, glyph: Arc::as_ptr(glyph) as usize };

        // Reuse the bin of this exact glyph; otherwise let the shelf pack
        // assign a fresh id.
        let existing = self.glyph_allocations.get(&key).map(|a| a.bin_id);
        let tex_handle = page.texture.reserve_size(size, existing);
        if let Some(tex_handle) = &tex_handle
            && existing.is_none()
        {
            self.glyph_allocations.insert(key, GlyphAllocation { _glyph: Arc::clone(glyph), bin_id: tex_handle.id() });
            self.glyph_allocation_keys.insert(BinKey { page: page.id, bin_id: tex_handle.id() }, key);
        }
        tex_handle
    }

    fn release_handles(&mut self, index: usize, texture_handles: &[TextureHandle]) {
        let page = &mut self.pages[index];
        for tex_handle in texture_handles {
            if !page.texture.remove_texture(tex_handle) {
                continue;
            }
            // The last handle was released, so forget the image revision or
            // glyph; its bin may now be reused.
            let bin_key = BinKey { page: page.id, bin_id: tex_handle.id() };
            if let Some(key) = self.image_allocation_keys.remove(&bin_key) {
                self.image_allocations.remove(&key);
            }
            if let Some(key) = self.glyph_allocation_keys.remove(&bin_key) {
                self.glyph_allocations.remove(&key);
            }
        }
    }

    /// Frees the bins of the leases dropped since, and the pages left
    /// without bins.
    pub(crate) fn free_released(&mut self) {
        let mut freed = false;
        while let Ok((page, texture_handles)) = self.released.try_recv() {
            if let Some(index) = self.pages.iter().position(|candidate| candidate.id == page) {
                self.release_handles(index, &texture_handles);
            } else if let Some(dummy) = self.dummies.iter_mut().find(|candidate| candidate.id == page) {
                for tex_handle in &texture_handles {
                    dummy.texture.remove_texture(tex_handle);
                }
            }
            freed = true;
        }
        if freed {
            self.pages.retain(|page| !page.texture.is_empty());
            self.dummies.retain(|page| !page.texture.is_empty());
        }
    }

    /// Copies `image` into the bins of every revision of it of the same size,
    /// so the layouts that draw them show it without laying out again.
    pub(crate) fn patch_image(&mut self, image: &ImageImpl) {
        for (key, allocation) in &self.image_allocations {
            if allocation.image.id != image.id || allocation.image.image.size != image.image.size {
                continue;
            }
            let Some(page) = self.pages.iter_mut().find(|page| page.id == key.page) else {
                continue;
            };
            if let Some(bin) = page.texture.bin(allocation.bin_id) {
                page.texture.queue_upload(allocation.bin_id, bin, padded_image(&image.image, bin, key.type_).data);
            }
        }
    }

    /// Makes the pages' GPU textures and uploads what was queued for them.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.free_released();
        for page in self.pages.iter_mut().chain(&mut self.dummies) {
            page.texture.upload(gpu, context);
        }
    }

    /// The GPU texture of the page `page`, once the atlas uploaded it.
    pub(crate) fn page_texture(&self, page: PageId) -> Option<&SharedTexture> {
        self.pages.iter().chain(&self.dummies).find(|candidate| candidate.id == page)?.texture.texture()
    }

    /// The pages, for tests.
    #[cfg(test)]
    pub(crate) fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The pixels queued for the page with id `page`, for tests.
    #[cfg(test)]
    pub(crate) fn queued_uploads(
        &self,
        page: PageId,
    ) -> Option<&HashMap<DynamicTextureBinId, (RectU16, QueuedPixels)>> {
        self.pages.iter().find(|candidate| candidate.id == page).map(|page| page.texture.queued_uploads())
    }
}
