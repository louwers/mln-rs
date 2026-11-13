// The C++ fixture creates a HeadlessBackend and a BackendScope only to hand
// the atlas a `gfx::Context`. The port's DynamicTextureAtlas takes no
// context (its dynamic textures create their GPU texture in the upload
// pass, see dynamic_texture.rs), so the fixture has no backend.

use std::sync::Arc;

use mln_renderer::gfx::dynamic_texture_atlas::{DynamicTextureAtlas, GlyphAtlas, ImageAtlas, ImageVersionMap};
use mln_style_types::image::{ImageMap, ImagePosition, Impl as ImageImpl};
use mln_text::glyph::{Glyph, GlyphID, GlyphMap, GlyphMetrics, Glyphs};
use mln_util::image::{AlphaImage, PremultipliedImage};
use mln_util::size::Size;

fn make_glyph(code: char) -> Arc<Glyph> {
    Arc::new(Glyph {
        id: GlyphID::new(code as u32),
        bitmap: AlphaImage::new(Size::new(8, 8)),
        metrics: GlyphMetrics { width: 8, height: 8, ..Default::default() },
    })
}

fn glyphs(glyph: &Arc<Glyph>) -> Glyphs {
    Glyphs::from([(glyph.id, Some(glyph.clone()))])
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
    ImageMap::from([(image.id.clone(), image.clone())])
}

struct DynamicTextureAtlasTest {
    atlas: DynamicTextureAtlas,
    atlases: Vec<ImageAtlas>,
    glyph_atlases: Vec<GlyphAtlas>,
}

impl DynamicTextureAtlasTest {
    fn new() -> Self {
        DynamicTextureAtlasTest { atlas: DynamicTextureAtlas::new(), atlases: Vec::new(), glyph_atlases: Vec::new() }
    }

    fn upload_glyphs(&mut self, glyphs: &GlyphMap) -> &GlyphAtlas {
        let glyph_atlas = self.atlas.upload_glyphs(glyphs);
        self.glyph_atlases.push(glyph_atlas);
        self.glyph_atlases.last().unwrap()
    }

    fn upload_pattern(&mut self, image: &Arc<ImageImpl>) -> &ImageAtlas {
        let image_atlas =
            self.atlas.upload_icons_and_patterns(&ImageMap::new(), &image_map(image), &ImageVersionMap::new());
        self.atlases.push(image_atlas);
        self.atlases.last().unwrap()
    }
}

impl Drop for DynamicTextureAtlasTest {
    fn drop(&mut self) {
        for image_atlas in &self.atlases {
            self.atlas.remove_textures(&image_atlas.texture_handles, &image_atlas.dynamic_texture);
        }
        for glyph_atlas in &self.glyph_atlases {
            self.atlas.remove_textures(&glyph_atlas.texture_handles, &glyph_atlas.dynamic_texture);
        }
    }
}

// gtest: DynamicTextureAtlasTest.SharesAllocationWithinImageRevision
#[test]
fn shares_allocation_within_image_revision() {
    let mut test = DynamicTextureAtlasTest::new();
    let image = make_image("pattern", false);

    let first = test.upload_pattern(&image).texture_handles[0];
    let second = test.upload_pattern(&image).texture_handles[0];

    assert_eq!(first.get_id(), second.get_id());
    assert_eq!(first.get_rectangle(), second.get_rectangle());
}

// gtest: DynamicTextureAtlasTest.ReplacedSameSizeImageGetsFreshAllocation
#[test]
fn replaced_same_size_image_gets_fresh_allocation() {
    let mut test = DynamicTextureAtlasTest::new();
    // Each revision keeps its layout alive, as a tile still rendering an older layout would.
    let original = test.upload_pattern(&make_image("pattern", false)).texture_handles[0];
    let sdf = test.upload_pattern(&make_image("pattern", true)).texture_handles[0];
    let toggled_back = test.upload_pattern(&make_image("pattern", false)).texture_handles[0];

    assert_ne!(original.get_rectangle(), sdf.get_rectangle());
    assert_ne!(original.get_rectangle(), toggled_back.get_rectangle());
    assert_ne!(sdf.get_rectangle(), toggled_back.get_rectangle());
}

// gtest: DynamicTextureAtlasTest.IconAndPatternUsageDoNotShareAllocation
#[test]
fn icon_and_pattern_usage_do_not_share_allocation() {
    let mut test = DynamicTextureAtlasTest::new();
    let image = make_image("image", false);

    let image_atlas =
        test.atlas.upload_icons_and_patterns(&image_map(&image), &image_map(&image), &ImageVersionMap::new());
    test.atlases.push(image_atlas);
    let image_atlas = test.atlases.last().unwrap();

    assert_eq!(2, image_atlas.texture_handles.len());
    assert_ne!(image_atlas.texture_handles[0].get_rectangle(), image_atlas.texture_handles[1].get_rectangle());
}

// gtest: DynamicTextureAtlasTest.ResizedImageWithSameAreaGetsItsOwnShape
#[test]
fn resized_image_with_same_area_gets_its_own_shape() {
    let mut test = DynamicTextureAtlasTest::new();
    let tall = make_sized_image("image", false, Size::new(10, 20));
    let wide = make_sized_image("image", false, Size::new(20, 10));

    // The layout of the tall image still holds its allocation while the wide one is laid out.
    test.upload_icon(&tall, 0);
    let position = &test.upload_icon(&wide, 0).icon_positions["image"];

    let padding = 2 * ImagePosition::PADDING;
    assert_eq!(u32::from(position.padded_rect.w - padding), wide.image.size.width);
    assert_eq!(u32::from(position.padded_rect.h - padding), wide.image.size.height);
}

// gtest: DynamicTextureAtlasTest.ReleasedRevisionGetsFreshAllocation
#[test]
fn released_revision_gets_fresh_allocation() {
    let mut test = DynamicTextureAtlasTest::new();
    let image = make_image("pattern", false);
    let released = test.atlas.upload_icons_and_patterns(&ImageMap::new(), &image_map(&image), &ImageVersionMap::new());
    let released_id = released.texture_handles[0].get_id();
    test.atlas.remove_textures(&released.texture_handles, &released.dynamic_texture);

    // The revision was forgotten on its last release, so it is packed and uploaded again under a new id.
    assert_ne!(released_id, test.upload_pattern(&image).texture_handles[0].get_id());
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
    let glyph_atlas = test.upload_glyphs(&GlyphMap::from([(0, glyphs(&b)), (4, glyphs(&a))]));

    assert_eq!(glyph_atlas.texture_handles.len(), 2);
    assert_ne!(glyph_atlas.texture_handles[0].get_rectangle(), glyph_atlas.texture_handles[1].get_rectangle());
    assert_ne!(glyph_atlas.glyph_positions[&0][&b.id].rect, glyph_atlas.glyph_positions[&4][&a.id].rect);
}

// gtest: DynamicTextureAtlasTest.FontStacksShareTheAllocationOfTheSameGlyph
#[test]
fn font_stacks_share_the_allocation_of_the_same_glyph() {
    let mut test = DynamicTextureAtlasTest::new();
    let glyph = make_glyph('A');
    let glyph_atlas = test.upload_glyphs(&GlyphMap::from([(1, glyphs(&glyph)), (2, glyphs(&glyph))]));

    assert_eq!(glyph_atlas.texture_handles.len(), 2);
    assert_eq!(glyph_atlas.texture_handles[0].get_id(), glyph_atlas.texture_handles[1].get_id());
}

impl DynamicTextureAtlasTest {
    /// Uploads `image` as an icon at `version`, keeping its atlas alive.
    fn upload_icon(&mut self, image: &Arc<ImageImpl>, version: u32) -> &ImageAtlas {
        let version_map = ImageVersionMap::from([(image.id.clone(), version)]);
        let image_atlas = self.atlas.upload_icons_and_patterns(&image_map(image), &ImageMap::new(), &version_map);
        self.atlases.push(image_atlas);
        self.atlases.last().unwrap()
    }
}

// Port-only: the icon cases of maplibre/maplibre-native#4751's
// DynamicTextureAtlas.UpdatedImageGetsSeparateRegion and
// ManyLiveVersionsGetSeparateRegions. Allocations here are keyed by image
// revision rather than by the version map, so a re-upload of the same
// revision shares its bin and every live revision gets its own.
#[test]
fn updated_icon_gets_fresh_allocation() {
    let mut test = DynamicTextureAtlasTest::new();
    let original_image = make_image("marker", false);
    let original = test.upload_icon(&original_image, 0).texture_handles[0];
    let shared = test.upload_icon(&original_image, 0).texture_handles[0];
    assert_eq!(original.get_id(), shared.get_id());

    let updated_atlas = test.upload_icon(&make_image("marker", false), 1);
    assert_eq!(updated_atlas.icon_positions["marker"].version, 1);
    let updated = updated_atlas.texture_handles[0];
    assert_ne!(original.get_rectangle(), updated.get_rectangle());
}

#[test]
fn many_live_icon_revisions_get_their_own_allocations() {
    const REVISIONS: u32 = 64;
    let mut test = DynamicTextureAtlasTest::new();
    let ids: std::collections::HashSet<i32> = (0..REVISIONS)
        .map(|version| test.upload_icon(&make_image("marker", false), version).texture_handles[0].get_id())
        .collect();
    assert_eq!(ids.len(), REVISIONS as usize);
}
