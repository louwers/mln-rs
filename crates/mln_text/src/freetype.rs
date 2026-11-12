// Ports src/mln/text/freetype.hpp and src/mln/text/freetype.cpp.
//
// `FreeTypeFace` keeps FreeType's semantics (face loading, charmap
// selection, sizing, advances) through the ported subset of FreeType in
// `crate::ft`; outlines come from skrifa (see `rasterize_glyph`).

use std::sync::{Arc, OnceLock};

use crate::ft;
use crate::glyph::{FontData, Glyph, GlyphID};

/// `SDF_FONT_SIZE`: glyphs are loaded at 24 px (`FT_Set_Char_Size` at 72 dpi).
pub const SDF_FONT_SIZE: i64 = 24;

/// `FreeTypeLibrary` (`FT_Init_FreeType` / `FT_Done_FreeType`). The port has no
/// library state; the type is kept because the C++ API passes it around.
#[derive(Debug, Default)]
pub struct FreeTypeLibrary;

impl FreeTypeLibrary {
    #[must_use]
    pub fn new() -> Self {
        FreeTypeLibrary
    }
}

/// `FreeTypeFace`.
#[derive(Clone)]
pub struct FreeTypeFace {
    /// `memoryFile`: the font data the face reads from, shared with whoever
    /// handed it over (such as the response it arrived in).
    memory_file: FontData,
    face: Option<ft::face::Face>,
    /// The hinted glyph loader (`FT_Load_Glyph` at 24 px), made on first use.
    scaler: Arc<OnceLock<Option<skrifa::ScalerFont>>>,
}

impl FreeTypeFace {
    /// `FreeTypeFace(const std::string& fontFileName, const FreeTypeLibrary&)`
    /// (`FT_New_Face` on a file).
    #[must_use]
    pub fn from_file(font_file_name: &str, lib: &FreeTypeLibrary) -> Self {
        match std::fs::read(font_file_name) {
            Ok(data) => Self::from_memory(data, lib),
            Err(_) => FreeTypeFace { memory_file: FontData::default(), face: None, scaler: Arc::default() },
        }
    }

    /// `FreeTypeFace(const char* fontData, size_t fontDataSize, const FreeTypeLibrary&)`.
    /// The face keeps `font_data` instead of copying it, unless it is a WOFF
    /// or WOFF2 file: then it keeps the font decoded from it, and isn't valid
    /// when that fails.
    pub fn from_memory(font_data: impl Into<FontData>, _lib: &FreeTypeLibrary) -> Self {
        let Ok(memory_file) = crate::woff::to_sfnt(font_data.into()) else {
            return FreeTypeFace { memory_file: FontData::default(), face: None, scaler: Arc::default() };
        };
        let face = match ft::face::Face::open(memory_file.as_bytes()) {
            Ok(mut face) => {
                Self::force_ucs2_charmap(&mut face);
                face.set_char_size(SDF_FONT_SIZE * 64, 72);
                Some(face)
            }
            Err(_) => None,
        };
        FreeTypeFace { memory_file, face, scaler: Arc::default() }
    }

    /// `force_ucs2_charmap`: select the first (0, 3) or (3, 1) charmap, if
    /// any; otherwise FreeType's default (`find_unicode_charmap`) stays.
    fn force_ucs2_charmap(face: &mut ft::face::Face) -> i32 {
        for i in 0..face.charmaps.len() {
            let cm = &face.charmaps[i];
            if (cm.platform_id == 0 && cm.encoding_id == 3) || (cm.platform_id == 3 && cm.encoding_id == 1) {
                return match face.set_charmap(i) {
                    Ok(()) => 0,
                    Err(_) => 6, // FT_Err_Invalid_Argument
                };
            }
        }
        -1
    }

    /// `isValid()`.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.face.is_some()
    }

    /// `getFace()`: the FreeType face state (None when not valid).
    #[must_use]
    pub fn get_face(&self) -> Option<&ft::face::Face> {
        self.face.as_ref()
    }

    /// The font data the face reads from.
    #[must_use]
    pub fn data(&self) -> &FontData {
        &self.memory_file
    }

    /// `FT_Get_Char_Index` on the selected charmap.
    #[must_use]
    pub fn get_char_index(&self, charcode: u32) -> u32 {
        let Some(face) = &self.face else { return 0 };
        let cm = face.charmap.map(|i| &face.charmaps[i]);
        ft::cmap::get_char_index(face.lookup_context(self.memory_file.as_bytes()), cm, charcode)
    }

    /// `FT_Face_GetCharVariantIndex`.
    #[must_use]
    pub fn get_char_variant_index(&self, charcode: u32, variant_selector: u32) -> u32 {
        let Some(face) = &self.face else { return 0 };
        ft::cmap::get_char_variant_index(
            face.lookup_context(self.memory_file.as_bytes()),
            &face.charmaps,
            face.charmap,
            charcode,
            variant_selector,
        )
    }

    /// `FT_Get_Advance(face, glyph, FT_LOAD_DEFAULT | FT_LOAD_NO_HINTING)`:
    /// the 16.16 advance, `None` on error.
    #[must_use]
    pub fn get_advance(&self, glyph: u32) -> Option<i64> {
        self.face.as_ref()?.get_advance(self.memory_file.as_bytes(), glyph)
    }

    /// The glyph loader of a valid face.
    pub(crate) fn scaler(&self) -> Option<&skrifa::ScalerFont> {
        self.face.as_ref()?;
        self.scaler.get_or_init(|| crate::rasterize::new_scaler(&self.memory_file)).as_ref()
    }

    /// The hinting engine `rasterize_glyph` uses (for tests and reports).
    #[must_use]
    pub fn hint_engine(&self) -> Option<skrifa::HintEngine> {
        self.scaler()?.hint_engine()
    }

    /// `rasterizeGlyph`.
    #[must_use]
    pub fn rasterize_glyph(&self, glyph: &GlyphID) -> Glyph {
        crate::rasterize::rasterize_glyph(self, *glyph)
    }
}
