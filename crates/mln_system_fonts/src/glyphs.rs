//! Local glyphs from the system's fonts, one character at a time.
//!
//! The font of a character, for a label in a font stack, is the first font
//! the stack names that the system has and that has the character, else the
//! font the system picks for it at the weight and style of the stack's first
//! name. Each name is looked up once, and a font the system picked for one
//! character of a class (`class`) serves the others of the class it has, so
//! that a map's labels look up a few fonts, not one for every character.

use std::sync::{Arc, LazyLock};

use mln_text::FontFile;
use mln_text::font_name::{self, FontName};
use mln_text::glyph::{Glyph, GlyphID};
use mln_text::local_glyph_rasterizer::LocalGlyphRasterizer;
use mln_util::font_stack::FontStack;
use mln_util::hash::{HashMap, HashSet};
use mln_util::i18n;
use parking_lot::Mutex;

use crate::source::{self, Source};
use crate::{Query, languages, system};

/// Draws local glyphs from the system's fonts (see the module docs). Every
/// rasterizer shares the fonts the process has found.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemGlyphRasterizer;

impl LocalGlyphRasterizer for SystemGlyphRasterizer {
    fn rasterize_glyph(&self, font_stack: &FontStack, glyph_id: GlyphID) -> Option<Glyph> {
        let c = char::from_u32(u32::from(glyph_id.code))?;
        font_for(font_stack, c)?.rasterize_char(c, glyph_id)
    }
}

/// The fonts found so far.
#[derive(Default)]
struct Fonts {
    files: source::Files,
    /// The font of each font name, if the system has it.
    named: HashMap<String, Option<Arc<FontFile>>>,
    /// The fonts the system picked for each font stack and class of
    /// characters, in the order found.
    picked: HashMap<(FontStack, u32), Vec<Arc<FontFile>>>,
    /// The characters that no font has, for each font stack.
    missing: HashSet<(FontStack, char)>,
}

static FONTS: LazyLock<Mutex<Fonts>> = LazyLock::new(Mutex::default);

/// The font that draws `c` for a label in `font_stack`.
fn font_for(font_stack: &FontStack, c: char) -> Option<Arc<FontFile>> {
    let mut fonts = FONTS.lock();
    let fonts = &mut *fonts;
    let names = names(font_stack);
    let mut named = Vec::with_capacity(names.len());
    for (name, parsed) in &names {
        let font = fonts.named.entry((*name).to_owned()).or_insert_with(|| {
            named_font(name, parsed).and_then(|(_, source)| FontFile::new(fonts.files.data(&source))).map(Arc::new)
        });
        named.push(font.clone());
    }
    let key = (font_stack.clone(), class(c));
    let picked = fonts.picked.get(&key).map_or(&[][..], Vec::as_slice);
    if let Some(font) = first_with(&named, picked, |font| font.has_char(c)) {
        return Some(Arc::clone(font));
    }
    if fonts.missing.contains(&(font_stack.clone(), c)) {
        return None;
    }
    let first = names.first().map_or(FontName { family: "", weight: 400, italic: false }, |(_, parsed)| *parsed);
    let font = picked_font(&first, c).and_then(|(_, source)| FontFile::new(fonts.files.data(&source))).map(Arc::new);
    match &font {
        Some(font) => fonts.picked.entry(key).or_default().push(Arc::clone(font)),
        None => {
            fonts.missing.insert((font_stack.clone(), c));
        }
    }
    font
}

/// The first font that has a character, as `has` says: of the stack's
/// `named` fonts in order, else of the fonts the system `picked`.
fn first_with<'a, F>(named: &'a [Option<F>], picked: &'a [F], has: impl Fn(&F) -> bool) -> Option<&'a F> {
    named.iter().flatten().chain(picked).find(|font| has(font))
}

/// The names of `font_stack` that name a font, with their parts.
fn names(font_stack: &FontStack) -> Vec<(&str, FontName<'_>)> {
    font_stack
        .iter()
        .filter(|name| !name.trim().is_empty())
        .map(|name| (name.as_str(), font_name::parse(name)))
        .collect()
}

/// The class of `c` whose fonts are found together: CJK ideographs, kana
/// and hangul (`i18n::allows_fixed_width_glyph_generation`), which a font
/// for the user's languages draws together; else the 256 code points
/// around it.
fn class(c: char) -> u32 {
    let ideograph = u16::try_from(u32::from(c)).is_ok_and(i18n::allows_fixed_width_glyph_generation);
    if ideograph { u32::MAX } else { u32::from(c) >> 8 }
}

/// The font that `name` names, if the system has its family, at the weight
/// closest to the name's, with its family name.
pub(crate) fn named_font(name: &str, parsed: &FontName<'_>) -> Option<(String, Source)> {
    // Every font has a space, so the lookup finds the family whatever it
    // draws.
    let query = Query {
        name: Some(name),
        family: Some(parsed.family),
        probe: ' ',
        languages: &[],
        weight: parsed.weight,
        italic: parsed.italic,
    };
    let candidates = drawn(system().finder.lock().candidates(&query), ' ');
    let named = candidates.iter().filter(|(family, ..)| family.eq_ignore_ascii_case(parsed.family));
    nearest(named, parsed.weight).map(|(family, source, _)| (family.clone(), source.clone()))
}

/// The font the platform picks for `c` in the family, weight and style of
/// `first`, the font stack's first name, with its family name.
pub(crate) fn picked_font(first: &FontName<'_>, c: char) -> Option<(String, Source)> {
    let system = system();
    let languages = if class(c) == u32::MAX { languages::for_ideographs(&system.preferred) } else { Vec::new() };
    let query = Query {
        name: None,
        family: Some(first.family).filter(|family| !family.is_empty()),
        probe: c,
        languages: &languages,
        weight: first.weight,
        italic: first.italic,
    };
    let candidates = drawn(system.finder.lock().candidates(&query), c);
    nearest(candidates.iter(), first.weight).map(|(family, source, _)| (family.clone(), source.clone()))
}

/// How many of a lookup's candidates to read.
const MAX_CANDIDATES: usize = 16;

/// The first of `candidates` that have `c`, with the weight they draw at.
fn drawn(candidates: Vec<(String, Source)>, c: char) -> Vec<(String, Source, u16)> {
    candidates
        .into_iter()
        .take(MAX_CANDIDATES)
        .filter_map(|(family, source)| {
            let weight = source.drawn_weight(c)?;
            Some((family, source, weight))
        })
        .collect()
}

/// The first of `candidates` that draws closest to `weight`.
fn nearest<'a>(
    candidates: impl Iterator<Item = &'a (String, Source, u16)>,
    weight: u16,
) -> Option<&'a (String, Source, u16)> {
    candidates.min_by_key(|(_, _, drawn)| drawn.abs_diff(weight))
}

#[cfg(test)]
mod tests {
    // Which fonts the system has depends on the machine, so the tests check
    // the fonts it finds, and pass with none; `--nocapture` lists them.

    use mln_text::font_name;
    use mln_text::glyph::GlyphID;
    use mln_text::local_glyph_rasterizer::LocalGlyphRasterizer;

    use super::{SystemGlyphRasterizer, class, first_with, named_font, picked_font};

    fn stack(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_picked_font_has_the_character() {
        for c in ['a', 'ä', 'Ж', 'α', 'א', 'ب', '中', 'あ', '한', 'ሀ'] {
            for name in ["", "Open Sans Regular", "Noto Sans Bold"] {
                let found = picked_font(&font_name::parse(name), c);
                println!("{c} for {name:?}: {found:?}");
                if let Some((_, source)) = found {
                    assert!(source.drawn_weight(c).is_some(), "{c} for {name:?}");
                }
            }
        }
    }

    #[test]
    fn a_name_gets_its_family_if_the_system_has_it() {
        // The family the platform picks for 'a' is one the system has.
        let Some((family, _)) = picked_font(&font_name::parse(""), 'a') else { return };
        let name = format!("{family} Bold");
        let (found, source) = named_font(&name, &font_name::parse(&name)).expect("the family");
        println!("{name}: {found} {source:?}");
        assert!(found.eq_ignore_ascii_case(&family));
        assert!(named_font("No Such Font Regular", &font_name::parse("No Such Font Regular")).is_none());
    }

    // A font the system picked, or a later name's, draws only what the
    // stack's earlier fonts lack, whatever was drawn before.
    #[test]
    fn the_stacks_fonts_come_first_in_order() {
        let ascii = Some("ab");
        let broad = Some("abé");
        let picked = ["abéz"];
        let has = |c: char| move |font: &&str| font.contains(c);
        assert_eq!(first_with(&[ascii, broad], &picked, has('a')), Some(&"ab"));
        assert_eq!(first_with(&[ascii, broad], &picked, has('é')), Some(&"abé"));
        assert_eq!(first_with(&[None, broad], &picked, has('a')), Some(&"abé"));
        assert_eq!(first_with(&[ascii, broad], &picked, has('z')), Some(&"abéz"));
        assert_eq!(first_with(&[ascii, broad], &picked, has('q')), None);
    }

    #[test]
    fn the_rasterizer_draws_what_a_font_has() {
        for c in ['a', '中', '한'] {
            let glyph =
                SystemGlyphRasterizer.rasterize_glyph(&stack(&["Open Sans Regular"]), GlyphID::new(u32::from(c)));
            println!("{c}: {:?}", glyph.as_ref().map(|glyph| glyph.metrics));
            if let Some(glyph) = glyph {
                assert_eq!(glyph.id, GlyphID::new(u32::from(c)));
                assert!(glyph.metrics.advance > 0);
                assert!(glyph.bitmap.data.iter().any(|&alpha| alpha > 0));
            }
        }
    }

    #[test]
    fn cjk_characters_share_a_class() {
        assert_eq!(class('中'), class('あ'));
        assert_eq!(class('中'), class('한'));
        assert_eq!(class('a'), class('ä'));
        assert_ne!(class('a'), class('Ж'));
    }
}
