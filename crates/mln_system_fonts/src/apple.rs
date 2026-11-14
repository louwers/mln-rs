//! The fonts for a script from CoreText: the default cascade list of the
//! system font for the script's languages
//! (`CTFontCopyDefaultCascadeListForLanguages`), the fonts that Apple's
//! platforms draw text in when the system font lacks a glyph. An iOS app
//! can't read the system's font files, so a font's bytes come from its
//! tables, through CoreText.

#![expect(unsafe_code, reason = "CoreText is the FFI here")]

use std::hash::{Hash, Hasher};
use std::ptr::NonNull;

use objc2_core_foundation::{CFArray, CFRetained, CFString};
use objc2_core_text::{CTFont, CTFontDescriptor, CTFontTableOptions, CTFontUIFontType};

use crate::scripts::Script;
use crate::sfnt::{self, Table};
use crate::source::Source;

/// A CoreText font, named by its PostScript name, which identifies it.
#[derive(Clone)]
pub(crate) struct Font {
    name: String,
    font: CFRetained<CTFont>,
}

// SAFETY: CoreText's font objects can be used from several threads at once
// ("Core Text Programming Guide: Thread Safety").
unsafe impl Send for Font {}
// SAFETY: as for `Send`.
unsafe impl Sync for Font {}

impl PartialEq for Font {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Font {}

impl Hash for Font {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl std::fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Font").field(&self.name).finish()
    }
}

impl Font {
    /// Its PostScript name.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Its tables `tags`, those it has.
    pub(crate) fn tables(&self, tags: &[[u8; 4]]) -> Vec<Table> {
        tags.iter().filter_map(|tag| Some((*tag, self.table(*tag)?))).collect()
    }

    fn table(&self, tag: [u8; 4]) -> Option<Vec<u8>> {
        // SAFETY: a valid font and tag.
        let data = unsafe { self.font.table(u32::from_be_bytes(tag), CTFontTableOptions::NoOptions) }?;
        Some(data.to_vec())
    }

    /// A font file of all its tables.
    pub(crate) fn file(&self) -> Vec<u8> {
        // SAFETY: a valid font.
        let Some(tags) = (unsafe { self.font.available_tables(CTFontTableOptions::NoOptions) }) else {
            return Vec::new();
        };
        let tables = (0..tags.count())
            .filter_map(|i| {
                // SAFETY: the array holds `CTFontTableTag`s as its values,
                // not objects, and `i` is within it.
                let tag = (unsafe { tags.value_at_index(i) } as usize as u32).to_be_bytes();
                Some((tag, self.table(tag)?))
            })
            .collect();
        sfnt::build(tables)
    }
}

/// The fonts that may draw `script` for `languages`, best first: those of the
/// cascade list that have its letter.
pub(crate) fn candidates(script: &Script, languages: &[String]) -> Vec<(String, Source)> {
    let languages: Vec<CFRetained<CFString>> = languages.iter().map(|tag| CFString::from_str(tag)).collect();
    let languages = CFArray::from_retained_objects(&languages);
    let mut probe = [0; 2];
    let probe = script.probe.encode_utf16(&mut probe);
    // SAFETY: the arguments are valid CoreText and CoreFoundation objects;
    // the cascade list holds font descriptors; the probe and its glyphs are
    // `probe.len()` long.
    unsafe {
        let Some(system) = CTFont::new_ui_font_for_language(CTFontUIFontType::System, 0.0, None) else {
            return Vec::new();
        };
        let Some(cascade) = system.default_cascade_list_for_languages(Some(languages.as_opaque())) else {
            return Vec::new();
        };
        let cascade: &CFArray<CTFontDescriptor> = cascade.cast_unchecked();
        cascade
            .iter()
            .filter_map(|descriptor| {
                let font = CTFont::with_font_descriptor(&descriptor, 0.0, std::ptr::null());
                let mut glyphs = [0; 2];
                let covers = font.glyphs_for_characters(
                    NonNull::new_unchecked(probe.as_mut_ptr()),
                    NonNull::new_unchecked(glyphs.as_mut_ptr()),
                    probe.len() as isize,
                );
                if !covers {
                    return None;
                }
                let family = font.family_name().to_string();
                let name = font.post_script_name().to_string();
                Some((family, Source::CoreText(Font { name, font })))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use skrifa::MetadataProvider;

    use super::candidates;
    use crate::scripts::SCRIPTS;
    use crate::source::Source;

    fn script(probe: char) -> &'static crate::scripts::Script {
        SCRIPTS.iter().find(|script| script.probe == probe).unwrap()
    }

    #[test]
    fn every_script_has_a_font_that_draws_it() {
        for script in SCRIPTS {
            let languages = script.language.map(|language| vec![language.to_owned()]).unwrap_or_default();
            let found = candidates(script, &languages);
            println!("{:04X}: {:?}", u32::from(script.probe), found.first());
            let (_, source) = found.first().unwrap_or_else(|| panic!("no font for {:04X}", u32::from(script.probe)));
            assert!(source.drawn_weight(script.probe).is_some());
        }
    }

    #[test]
    fn ideographs_take_a_font_for_the_users_language() {
        let han = script('\u{4E2D}');
        let first = |language: &str| candidates(han, &[language.to_owned()]).remove(0);
        let (japanese, chinese) = (first("ja-JP"), first("zh-Hans"));
        println!("ja-JP: {japanese:?}\nzh-Hans: {chinese:?}");
        assert_ne!(japanese.1, chinese.1);
    }

    #[test]
    fn a_font_file_from_tables_reads_back() {
        let (_, Source::CoreText(font)) = candidates(script('\u{4E2D}'), &["ja-JP".to_owned()]).remove(0) else {
            panic!("a CoreText font");
        };
        let file = font.file();
        let parsed = skrifa::FontRef::new(&file).expect("a font");
        assert!(parsed.charmap().map('\u{4E2D}').is_some());
    }
}
