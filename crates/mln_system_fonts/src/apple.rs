//! The fonts for a character from CoreText: the font asked for by name, then
//! the default cascade list of that font, or of the system font, for the
//! text's languages (`CTFontCopyDefaultCascadeListForLanguages`), the fonts
//! that Apple's platforms draw text in when a font lacks a glyph. An iOS app
//! can't read the system's font files, so a font's bytes come from its
//! tables, through CoreText.

#![expect(unsafe_code, reason = "CoreText is the FFI here")]

use std::hash::{Hash, Hasher};
use std::ptr::NonNull;

use mln_style::hash::HashMap;
use objc2_core_foundation::{CFArray, CFRetained, CFString};
use objc2_core_text::{CTFont, CTFontDescriptor, CTFontSymbolicTraits, CTFontTableOptions, CTFontUIFontType};

use crate::Query;
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
        sfnt::build(table_tags(&self.font).into_iter().filter_map(|tag| Some((tag, self.table(tag)?))).collect())
    }
}

/// The tags of `font`'s tables.
fn table_tags(font: &CTFont) -> Vec<[u8; 4]> {
    // SAFETY: a valid font.
    let Some(tags) = (unsafe { font.available_tables(CTFontTableOptions::NoOptions) }) else {
        return Vec::new();
    };
    (0..tags.count())
        .map(|i| {
            // SAFETY: the array holds `CTFontTableTag`s as its values, not
            // objects, and `i` is within it.
            (unsafe { tags.value_at_index(i) } as usize as u32).to_be_bytes()
        })
        .collect()
}

/// Whether `font` has outlines that the map draws: `glyf`, `CFF ` or
/// `CFF2`. Some of the system's fonts have only Apple's own, and reading
/// their tables to find out would copy tens of megabytes each.
fn has_outlines(font: &CTFont) -> bool {
    table_tags(font).iter().any(|tag| matches!(tag, b"glyf" | b"CFF " | b"CFF2"))
}

/// CoreText's font matcher. It keeps each cascade list it makes, which
/// takes milliseconds, since a map asks for one for every character that
/// the fonts it has found lack.
#[derive(Default)]
pub(crate) struct CoreText {
    /// The fonts of the cascade list of a font, by its PostScript name, for
    /// languages and symbolic traits, with their family names.
    cascades: HashMap<(String, Vec<String>, u32), Cascade>,
}

/// The fonts of a cascade list, with their family names.
type Cascade = Vec<(String, Font)>;

impl CoreText {
    /// The fonts that may draw `query`'s character, best first: the font of
    /// its name or family, if the system has it, then those of the cascade list
    /// that have the character, at its weight and style where they have them.
    pub(crate) fn candidates(&mut self, query: &Query<'_>) -> Vec<(String, Source)> {
        let mut units = [0; 2];
        let length = query.probe.encode_utf16(&mut units).len();
        let covers = |font: &CTFont| {
            let (mut probe, mut glyphs) = (units, [0; 2]);
            let probe = &mut probe[..length];
            // SAFETY: the probe and its glyphs are `probe.len()` long.
            unsafe {
                font.glyphs_for_characters(
                    NonNull::new_unchecked(probe.as_mut_ptr()),
                    NonNull::new_unchecked(glyphs.as_mut_ptr()),
                    probe.len() as isize,
                )
            }
        };
        let mut traits = CTFontSymbolicTraits::empty();
        if query.weight >= 600 {
            traits |= CTFontSymbolicTraits::TraitBold;
        }
        if query.italic {
            traits |= CTFontSymbolicTraits::TraitItalic;
        }
        let source = |font: CFRetained<CTFont>| -> (String, Source) {
            // SAFETY: a valid font.
            let (family, name) = unsafe { (font.family_name().to_string(), font.post_script_name().to_string()) };
            (family, Source::CoreText(Font { name, font }))
        };

        // CoreText finds a font by its PostScript or full name, and falls back to
        // another for a name it lacks.
        let named = [query.name, query.family].into_iter().flatten().find_map(|name| {
            // SAFETY: a valid name and a null matrix.
            let font = unsafe { CTFont::with_name(&CFString::from_str(name), 0.0, std::ptr::null()) };
            // SAFETY: a valid font.
            let family = unsafe { font.family_name() }.to_string();
            query.family.is_some_and(|asked| asked.eq_ignore_ascii_case(&family)).then_some(font)
        });
        let base = match &named {
            Some(font) => font.clone(),
            // SAFETY: a valid font type and no language.
            None => match unsafe { CTFont::new_ui_font_for_language(CTFontUIFontType::System, 0.0, None) } {
                Some(font) => font,
                None => return Vec::new(),
            },
        };
        let mut candidates: Vec<(String, Source)> = Vec::new();
        if let Some(named) = named.filter(|font| covers(font)) {
            candidates.push(source(named));
        }
        // SAFETY: a valid font.
        let key = (unsafe { base.post_script_name() }.to_string(), query.languages.to_vec(), traits.0);
        let cascade = self.cascades.entry(key).or_insert_with(|| cascade(&base, query.languages, traits));
        candidates.extend(
            cascade
                .iter()
                .filter(|(_, font)| covers(&font.font))
                .map(|(family, font)| (family.clone(), Source::CoreText(font.clone()))),
        );
        candidates
    }
}

/// The fonts of `base`'s default cascade list for `languages` that have
/// outlines the map draws, with `traits` where their families have them, and
/// their family names.
fn cascade(base: &CTFont, languages: &[String], traits: CTFontSymbolicTraits) -> Cascade {
    let languages: Vec<CFRetained<CFString>> = languages.iter().map(|tag| CFString::from_str(tag)).collect();
    let languages = CFArray::from_retained_objects(&languages);
    // SAFETY: a valid font and null matrix; a copy with traits the font's
    // family lacks is `None`.
    let styled = |font: CFRetained<CTFont>| -> CFRetained<CTFont> {
        if traits.is_empty() {
            return font;
        }
        unsafe { font.copy_with_symbolic_traits(0.0, std::ptr::null(), traits, traits) }.unwrap_or(font)
    };
    // SAFETY: the arguments are valid CoreText and CoreFoundation objects;
    // the cascade list holds font descriptors.
    unsafe {
        let Some(cascade) = base.default_cascade_list_for_languages(Some(languages.as_opaque())) else {
            return Vec::new();
        };
        let cascade: &CFArray<CTFontDescriptor> = cascade.cast_unchecked();
        cascade
            .iter()
            .map(|descriptor| styled(CTFont::with_font_descriptor(&descriptor, 0.0, std::ptr::null())))
            .filter(|font| has_outlines(font))
            .map(|font| {
                let (family, name) = (font.family_name().to_string(), font.post_script_name().to_string());
                (family, Font { name, font })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use skrifa::MetadataProvider;

    use super::{CoreText, has_outlines};
    use crate::Query;
    use crate::scripts::SCRIPTS;
    use crate::source::Source;

    fn query(probe: char, languages: &[String]) -> Query<'_> {
        Query { name: None, family: None, probe, languages, weight: 400, italic: false }
    }

    #[test]
    fn every_script_has_a_font_that_draws_it() {
        for script in SCRIPTS {
            let languages = [script.language.to_owned()];
            let found = CoreText::default().candidates(&query(script.probe, &languages));
            println!("{:04X}: {:?}", u32::from(script.probe), found.first());
            let (_, source) = found.first().unwrap_or_else(|| panic!("no font for {:04X}", u32::from(script.probe)));
            assert!(source.drawn_weight(script.probe, 400).is_some());
            let outlined =
                |(_, source): &(String, Source)| matches!(source, Source::CoreText(font) if has_outlines(&font.font));
            assert!(found.iter().all(outlined), "a font without outlines for {:04X}", u32::from(script.probe));
        }
    }

    /// Each language's first font draws the ideograph. Chinese and Japanese
    /// can get the same font: the Chinese font of CoreText's cascade list
    /// may have only Apple's `hvgl` outlines, which the map can't read
    /// (<https://github.com/maplibre/mln-rs/issues/737>).
    #[test]
    fn ideographs_take_a_font_that_draws_them() {
        for language in ["ja-JP", "zh-Hans"] {
            let first = CoreText::default().candidates(&query('\u{4E2D}', &[language.to_owned()])).remove(0);
            println!("{language}: {first:?}");
            let (_, Source::CoreText(font)) = first else { panic!("a CoreText font") };
            let file = font.file();
            let parsed = skrifa::FontRef::new(&file).expect("a font");
            let glyph = parsed.charmap().map('\u{4E2D}').expect("a glyph for the ideograph");
            assert!(parsed.outline_glyphs().get(glyph).is_some(), "{language}: no outline for the ideograph");
        }
    }

    #[test]
    fn a_font_asked_for_by_name_comes_first() {
        let found = CoreText::default().candidates(&Query {
            name: Some("Helvetica Bold"),
            family: Some("Helvetica"),
            ..query('a', &[])
        });
        println!("{:?}", found.first());
        assert_eq!(found.first().map(|(family, _)| family.as_str()), Some("Helvetica"));
    }

    #[test]
    fn a_font_file_from_tables_reads_back() {
        let (_, Source::CoreText(font)) =
            CoreText::default().candidates(&query('\u{4E2D}', &["ja-JP".to_owned()])).remove(0)
        else {
            panic!("a CoreText font");
        };
        let file = font.file();
        let parsed = skrifa::FontRef::new(&file).expect("a font");
        assert!(parsed.charmap().map('\u{4E2D}').is_some());
    }
}
