// The C++ fixture creates a HeadlessBackend and a BackendScope only to hand
// the atlas a `gfx::Context`. The port's DynamicTextureAtlas takes no
// context (its dynamic textures create their GPU texture in the upload
// pass, see dynamic_texture.rs), so the fixture has no backend.

use std::sync::Arc;

use mln_style::bitmap::{AlphaImage, PremultipliedImage};
use mln_style::image::Impl as ImageImpl;
use mln_style::size::Size;
use mln_text::atlas::ImagePosition;
use mln_text::glyph::{Glyph, GlyphID, GlyphMap, GlyphMetrics, Glyphs};

use crate::draw::dynamic_texture::{DynamicTextureBinId, QueuedPixels, TextureHandle};
use crate::draw::dynamic_texture_atlas::{DynamicTextureAtlas, GlyphPage, ImagePage};
use crate::image::image_store::ImageMap;

fn make_glyph(code: char) -> Arc<Glyph> {
    Arc::new(Glyph {
        id: GlyphID::new(code as u32),
        bitmap: AlphaImage::new(Size::new(8, 8)),
        metrics: GlyphMetrics { width: 8, height: 8, ..Default::default() },
    })
}

fn glyphs(glyph: &Arc<Glyph>) -> Glyphs {
    Glyphs::from([(glyph.id, Some(Arc::clone(glyph)))])
}

/// A 16×16 image named `id`.
fn make_image(id: &str, sdf: bool) -> Arc<ImageImpl> {
    make_sized_image(id, sdf, Size::new(16, 16))
}

fn make_sized_image(id: &str, sdf: bool, size: Size) -> Arc<ImageImpl> {
    Arc::new(
        ImageImpl::new(
            id.to_owned(),
            PremultipliedImage::new(size),
            1.0,
            sdf,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .expect("image"),
    )
}

fn image_map(image: &Arc<ImageImpl>) -> ImageMap {
    ImageMap::from([(image.id.clone(), Arc::clone(image))])
}

/// An atlas, and the pages it packed, which keep their bins as a tile's
/// layout would.
struct DynamicTextureAtlasTest {
    atlas: DynamicTextureAtlas,
    image_pages: Vec<ImagePage>,
    glyph_pages: Vec<GlyphPage>,
}

impl DynamicTextureAtlasTest {
    fn new() -> Self {
        DynamicTextureAtlasTest { atlas: DynamicTextureAtlas::new(), image_pages: Vec::new(), glyph_pages: Vec::new() }
    }

    fn upload_glyphs(&mut self, glyphs: &GlyphMap) -> &GlyphPage {
        let page = self.atlas.pack_glyphs(glyphs);
        self.glyph_pages.push(page);
        self.glyph_pages.last().unwrap()
    }

    fn upload_images(&mut self, icons: &ImageMap, patterns: &ImageMap) -> &ImagePage {
        let page = self.atlas.pack_images(icons, patterns);
        self.image_pages.push(page);
        self.image_pages.last().unwrap()
    }

    fn upload_pattern(&mut self, image: &Arc<ImageImpl>) -> &ImagePage {
        self.upload_images(&ImageMap::new(), &image_map(image))
    }

    /// Uploads `image` as an icon, keeping its page alive.
    fn upload_icon(&mut self, image: &Arc<ImageImpl>) -> &ImagePage {
        self.upload_images(&image_map(image), &ImageMap::new())
    }
}

fn handle(page: &ImagePage) -> TextureHandle {
    page.lease.texture_handles()[0]
}

// gtest: DynamicTextureAtlasTest.SharesAllocationWithinImageRevision
#[test]
fn shares_allocation_within_image_revision() {
    let mut test = DynamicTextureAtlasTest::new();
    let image = make_image("pattern", false);

    let first = handle(test.upload_pattern(&image));
    let second = handle(test.upload_pattern(&image));

    assert_eq!(first.id(), second.id());
    assert_eq!(first.rectangle(), second.rectangle());
}

// gtest: DynamicTextureAtlasTest.ReplacedSameSizeImageGetsFreshAllocation
#[test]
fn replaced_same_size_image_gets_fresh_allocation() {
    let mut test = DynamicTextureAtlasTest::new();
    // Each revision keeps its layout alive, as a tile still rendering an older layout would.
    let original = handle(test.upload_pattern(&make_image("pattern", false)));
    let sdf = handle(test.upload_pattern(&make_image("pattern", true)));
    let toggled_back = handle(test.upload_pattern(&make_image("pattern", false)));

    assert_ne!(original.rectangle(), sdf.rectangle());
    assert_ne!(original.rectangle(), toggled_back.rectangle());
    assert_ne!(sdf.rectangle(), toggled_back.rectangle());
}

// gtest: DynamicTextureAtlasTest.IconAndPatternUsageDoNotShareAllocation
#[test]
fn icon_and_pattern_usage_do_not_share_allocation() {
    let mut test = DynamicTextureAtlasTest::new();
    let image = make_image("image", false);

    let page = test.upload_images(&image_map(&image), &image_map(&image));

    let handles = page.lease.texture_handles();
    assert_eq!(2, handles.len());
    assert_ne!(handles[0].rectangle(), handles[1].rectangle());
}

// gtest: DynamicTextureAtlasTest.ResizedImageWithSameAreaGetsItsOwnShape
#[test]
fn resized_image_with_same_area_gets_its_own_shape() {
    let mut test = DynamicTextureAtlasTest::new();
    let tall = make_sized_image("image", false, Size::new(10, 20));
    let wide = make_sized_image("image", false, Size::new(20, 10));

    // The layout of the tall image still holds its allocation while the wide one is laid out.
    test.upload_icon(&tall);
    let position = &test.upload_icon(&wide).icons["image"];

    let padding = 2 * ImagePosition::PADDING;
    assert_eq!(u32::from(position.padded_rect.w - padding), wide.image.size.width);
    assert_eq!(u32::from(position.padded_rect.h - padding), wide.image.size.height);
}

// gtest: DynamicTextureAtlasTest.ReleasedRevisionGetsFreshAllocation
#[test]
fn released_revision_gets_fresh_allocation() {
    let mut test = DynamicTextureAtlasTest::new();
    // Keeps the page.
    test.upload_icon(&make_image("other", false));
    let image = make_image("pattern", false);
    let released = test.atlas.pack_images(&ImageMap::new(), &image_map(&image));
    let released_id = handle(&released).id();
    drop(released);

    // The revision was forgotten on its last release, so it is packed and uploaded again under a new id.
    let page = test.upload_pattern(&image);
    assert_ne!(released_id, handle(page).id());
}

// gtest: DynamicTextureAtlasTest.GlyphsOfDifferentFontStacksGetTheirOwnAllocations
#[test]
fn glyphs_of_different_font_stacks_get_their_own_allocations() {
    let mut test = DynamicTextureAtlasTest::new();
    // These font stack hashes and code points once gave both glyphs the same
    // bin, as a 32-bit build's hashes often did for Latin and CJK glyphs, so
    // one drew the other.
    let b = make_glyph('B');
    let a = make_glyph('A');
    let page = test.upload_glyphs(&GlyphMap::from([(0, glyphs(&b)), (4, glyphs(&a))]));

    let handles = page.lease.texture_handles();
    assert_eq!(handles.len(), 2);
    assert_ne!(handles[0].rectangle(), handles[1].rectangle());
    assert_ne!(page.positions[&0][&b.id].unwrap().rect, page.positions[&4][&a.id].unwrap().rect);
}

// gtest: DynamicTextureAtlasTest.FontStacksShareTheAllocationOfTheSameGlyph
#[test]
fn font_stacks_share_the_allocation_of_the_same_glyph() {
    let mut test = DynamicTextureAtlasTest::new();
    let glyph = make_glyph('A');
    let page = test.upload_glyphs(&GlyphMap::from([(1, glyphs(&glyph)), (2, glyphs(&glyph))]));

    let handles = page.lease.texture_handles();
    assert_eq!(handles.len(), 2);
    assert_eq!(handles[0].id(), handles[1].id());
}

/// A glyph no font has stays missing, and one without a bitmap has its
/// metrics and no bin.
#[test]
fn missing_and_blank_glyphs_take_no_bin() {
    let mut test = DynamicTextureAtlasTest::new();
    let drawn = make_glyph('A');
    let blank = Arc::new(Glyph {
        id: GlyphID::new(' ' as u32),
        bitmap: AlphaImage::default(),
        metrics: GlyphMetrics { advance: 6, ..Default::default() },
    });
    let mut font = glyphs(&drawn);
    font.insert(blank.id, Some(Arc::clone(&blank)));
    font.insert(GlyphID::new('B' as u32), None);
    let page = test.upload_glyphs(&GlyphMap::from([(1, font)]));

    assert_eq!(page.lease.texture_handles().len(), 1);
    let positions = &page.positions[&1];
    assert!(positions[&GlyphID::new('B' as u32)].is_none());
    let blank_position = positions[&blank.id].unwrap();
    assert_eq!((blank_position.rect.w, blank_position.metrics.advance), (0, 6));
    assert_ne!(positions[&drawn.id].unwrap().rect.w, 0);
}

/// Dropping the last lease on a page's bins frees them, and the page with
/// them, the next time the atlas packs or uploads.
#[test]
fn dropped_leases_free_their_bins_and_empty_pages() {
    let mut atlas = DynamicTextureAtlas::new();
    let image = make_image("icon", false);
    let first = atlas.pack_images(&image_map(&image), &ImageMap::new());
    let second = atlas.pack_images(&image_map(&image), &ImageMap::new());
    assert_eq!(first.lease.page(), second.lease.page());
    assert_eq!(atlas.page_count(), 1);

    drop(first);
    atlas.free_released();
    assert_eq!(atlas.page_count(), 1, "the second lease still holds the bin");
    drop(second);
    atlas.free_released();
    assert_eq!(atlas.page_count(), 0);

    // An empty pack takes the bin of a 1×1 page, which isn't packed into,
    // and goes with its last lease.
    let empty = atlas.pack_images(&ImageMap::new(), &ImageMap::new());
    assert_eq!(empty.lease.texture_handles().len(), 1);
    let second = atlas.pack_images(&ImageMap::new(), &ImageMap::new());
    assert_eq!(second.lease.page(), empty.lease.page());
    assert_eq!(atlas.page_count(), 0);
    let page = empty.lease.page();
    drop((empty, second));
    atlas.free_released();
    assert_ne!(atlas.pack_images(&ImageMap::new(), &ImageMap::new()).lease.page(), page);
}

// Port-only: the icon cases of maplibre/maplibre-native#4751's
// DynamicTextureAtlas.UpdatedImageGetsSeparateRegion and
// ManyLiveVersionsGetSeparateRegions. Allocations here are keyed by image
// revision, so a re-upload of the same revision shares its bin and every
// live revision gets its own.
#[test]
fn updated_icon_gets_fresh_allocation() {
    let mut test = DynamicTextureAtlasTest::new();
    let original_image = make_image("marker", false);
    let original = handle(test.upload_icon(&original_image));
    let shared = handle(test.upload_icon(&original_image));
    assert_eq!(original.id(), shared.id());

    let updated = handle(test.upload_icon(&make_image("marker", false)));
    assert_ne!(original.rectangle(), updated.rectangle());
}

#[test]
fn many_live_icon_revisions_get_their_own_allocations() {
    const REVISIONS: u32 = 64;
    let mut test = DynamicTextureAtlasTest::new();
    let ids: mln_style::hash::HashSet<DynamicTextureBinId> =
        (0..REVISIONS).map(|_| handle(test.upload_icon(&make_image("marker", false))).id()).collect();
    assert_eq!(ids.len(), REVISIONS as usize);
}

/// A 16×16 image named `id` whose pixel (x, y) is `[x, y, 0, 255]`.
fn make_gradient_image(id: &str) -> Arc<ImageImpl> {
    let mut image = PremultipliedImage::new(Size::new(16, 16));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        *pixel = [(i % 16) as u8, (i / 16) as u8, 0, 255];
    }
    Arc::new(ImageImpl::new(id.to_owned(), image, 1.0, false, Vec::new(), Vec::new(), None, None, None).expect("image"))
}

// Port-only: a same-size image update copies the new pixels into the bins of
// the revisions that layouts hold, with a pattern's wrapped edges.
#[test]
fn patched_pattern_is_queued_for_its_bin() {
    let mut test = DynamicTextureAtlasTest::new();
    test.upload_pattern(&make_image("pattern", false));
    let page = test.image_pages[0].lease.page();
    let tex_handle = handle(&test.image_pages[0]);
    let queued = |test: &DynamicTextureAtlasTest| test.atlas.queued_uploads(page).unwrap()[&tex_handle.id()].clone();
    let original = queued(&test);

    test.atlas.patch_image(&make_sized_image("pattern", false, Size::new(8, 8)));
    test.atlas.patch_image(&make_gradient_image("other"));
    assert_eq!(queued(&test), original);

    test.atlas.patch_image(&make_gradient_image("pattern"));
    let (rect, QueuedPixels::Bytes(data)) = queued(&test) else {
        panic!("a patch queues the bin's pixels");
    };
    assert_eq!(&rect, tex_handle.rectangle());

    // The image starts two pixels in: one of padding, one of wrapped edge.
    let pixel = |x: usize, y: usize| {
        let i = (y * usize::from(rect.w) + x) * 4;
        &data[i..i + 4]
    };
    assert_eq!(pixel(2 + 3, 2 + 5), [3, 5, 0, 255]);
    assert_eq!(pixel(1, 2 + 5), [15, 5, 0, 255]);
    assert_eq!(pixel(2 + 3, 2 + 16), [3, 0, 0, 255]);
}
