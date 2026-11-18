use crate::gfx::dynamic_texture_atlas::{DynamicTextureAtlas, PageId};
use crate::gfx::texture2d::TextureKey;

/// The glyph and icon (+ pattern) atlas textures of a tile's layout: the
/// pages its glyphs and images are on, and their GPU textures, which the
/// atlas makes in the frame's upload.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileAtlasTextures {
    glyph_page: Option<PageId>,
    icon_page: Option<PageId>,
    glyph: Option<TextureKey>,
    icon: Option<TextureKey>,
}

impl TileAtlasTextures {
    /// The textures of the pages `glyph` and `icon`, which `resolve` finds.
    pub(crate) fn new(glyph: PageId, icon: PageId) -> Self {
        TileAtlasTextures { glyph_page: Some(glyph), icon_page: Some(icon), glyph: None, icon: None }
    }

    /// Finds the pages' textures in `atlas`, once it uploaded them.
    pub(crate) fn resolve(&mut self, atlas: &DynamicTextureAtlas) {
        if self.glyph.is_none() {
            self.glyph = self.glyph_page.and_then(|page| atlas.page_texture(page));
        }
        if self.icon.is_none() {
            self.icon = self.icon_page.and_then(|page| atlas.page_texture(page));
        }
    }

    /// The glyph page's texture, once uploaded.
    pub(crate) fn glyph(&self) -> Option<TextureKey> {
        self.glyph
    }

    /// The icon and pattern page's texture, once uploaded.
    pub(crate) fn icon(&self) -> Option<TextureKey> {
        self.icon
    }
}
