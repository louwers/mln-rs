use crate::gfx::dynamic_texture::PageTexture;
use crate::gfx::texture2d::Texture2DPtr;

/// The glyph and icon (+ pattern) atlas textures of a tile's layout: the
/// pages its glyphs and images are on, whose GPU textures the atlas makes in
/// the frame's upload.
#[derive(Default)]
pub(crate) struct TileAtlasTextures {
    glyph: Option<PageTexture>,
    icon: Option<PageTexture>,
}

impl TileAtlasTextures {
    pub(crate) fn new(glyph: PageTexture, icon: PageTexture) -> Self {
        TileAtlasTextures { glyph: Some(glyph), icon: Some(icon) }
    }

    /// The glyph page's texture, once uploaded.
    pub(crate) fn glyph(&self) -> Option<Texture2DPtr> {
        self.glyph.as_ref().and_then(|texture| texture.get().cloned())
    }

    /// The icon and pattern page's texture, once uploaded.
    pub(crate) fn icon(&self) -> Option<Texture2DPtr> {
        self.icon.as_ref().and_then(|texture| texture.get().cloned())
    }
}

impl std::fmt::Debug for TileAtlasTextures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileAtlasTextures")
            .field("glyph", &self.glyph().is_some())
            .field("icon", &self.icon().is_some())
            .finish()
    }
}
