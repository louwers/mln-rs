//! The system's font for each cluster of a label, a character and the marks
//! and joiners after it, which shapes it or draws its glyph.
//!
//! The font of a cluster, for a label in a font stack, is the first font the
//! stack names that the system has and that draws the whole cluster, else
//! the font the system picks for it at the weight and style of the stack's
//! first name, else the font of its first character alone. Each name is
//! looked up once, and a font the system picked for one character of a class
//! (`class`) serves the others of the class it has, so that a map's labels
//! look up a few fonts, not one for every character.

use std::sync::Arc;

use mln_style::hash::{HashMap, HashSet};
use mln_style::i18n;

use crate::text::FontFile;
use crate::text::font_name::{self, FontName};
use crate::text::font_stack::FontStack;
use crate::text::glyph::FontFace;
use crate::text::local_fonts::FontChooser;
use crate::text::system_fonts::source::{self, Source};
use crate::text::system_fonts::{Finder, Query, languages, system};

/// The system's fonts as a [`FontChooser`] (see the module docs). Every
/// chooser shares the fonts the process has found.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemFonts;

impl FontChooser for SystemFonts {
    fn choose(&self, font_stack: &FontStack, cluster: &[char]) -> Option<FontFace> {
        let mut fonts = system().fonts.as_ref()?.lock();
        Some(font_for(&mut fonts, font_stack, cluster)?.face.clone())
    }
}

/// A font the system has, at the weight it was found for.
pub(crate) struct SystemFont {
    file: FontFile,
    face: FontFace,
}

impl SystemFont {
    fn draws(&self, cluster: &[char]) -> bool {
        cluster.iter().all(|&c| self.file.has_char(c))
    }
}

/// The platform's font matcher, and the fonts found with it so far.
pub(crate) struct Fonts {
    pub(crate) finder: Finder,
    files: source::Files,
    /// The font of each font name, if the system has it.
    named: HashMap<String, Option<Arc<SystemFont>>>,
    /// The fonts the system picked for each font stack and class of
    /// characters, in the order found.
    picked: HashMap<(FontStack, u32), Vec<Arc<SystemFont>>>,
    /// The clusters that no font draws whole, for each font stack.
    missing: HashSet<(FontStack, Vec<char>)>,
}

impl Fonts {
    /// No fonts found yet with `finder`.
    pub(crate) fn new(finder: Finder) -> Self {
        Fonts {
            finder,
            files: source::Files::default(),
            named: HashMap::default(),
            picked: HashMap::default(),
            missing: HashSet::default(),
        }
    }
}

/// The font that draws `cluster` for a label in `font_stack` (see the module
/// docs).
fn font_for(fonts: &mut Fonts, font_stack: &FontStack, cluster: &[char]) -> Option<Arc<SystemFont>> {
    let &c = cluster.first()?;
    let names = names(font_stack);
    let mut named = Vec::with_capacity(names.len());
    for (name, parsed) in &names {
        let font = fonts.named.entry((*name).to_owned()).or_insert_with(|| {
            first_drawing(&mut fonts.files, named_fonts(&mut fonts.finder, name, parsed), &[' '], parsed.weight)
        });
        named.push(font.clone());
    }
    let key = (font_stack.clone(), class(c));
    let picked = fonts.picked.get(&key).map_or(&[][..], Vec::as_slice);
    if let Some(font) = first_with(&named, picked, |font| font.draws(cluster)) {
        return Some(Arc::clone(font));
    }
    if !fonts.missing.contains(&(font_stack.clone(), cluster.to_vec())) {
        let first = names.first().map_or(FontName { family: "", weight: 400, italic: false }, |(_, parsed)| *parsed);
        let candidates = picked_fonts(&mut fonts.finder, &first, c);
        if let Some(font) = first_drawing(&mut fonts.files, candidates, cluster, first.weight) {
            fonts.picked.entry(key).or_default().push(Arc::clone(&font));
            return Some(font);
        }
        fonts.missing.insert((font_stack.clone(), cluster.to_vec()));
    }
    // A mark that no font draws with its character goes with the
    // character's font, which draws what it can.
    if cluster.len() > 1 { font_for(fonts, font_stack, &[c]) } else { None }
}

/// The first font that has a character, as `has` says: of the stack's
/// `named` fonts in order, else of the fonts the system `picked`.
fn first_with<'a, F>(named: &'a [Option<F>], picked: &'a [F], has: impl Fn(&F) -> bool) -> Option<&'a F> {
    named.iter().flatten().chain(picked).find(|font| has(font))
}

/// The first of `candidates` that the map can draw `cluster` from, as a face
/// at `weight`.
fn first_drawing(
    files: &mut source::Files,
    candidates: Vec<(String, Source)>,
    cluster: &[char],
    weight: u16,
) -> Option<Arc<SystemFont>> {
    candidates.into_iter().find_map(|(_, source)| {
        let data = files.data(&source)?;
        let file = FontFile::new(data.clone())?;
        let face = FontFace::device(&source.url(), weight, data);
        let font = SystemFont { file, face };
        font.draws(cluster).then(|| Arc::new(font))
    })
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
    let ideograph = i18n::allows_fixed_width_glyph_generation(c);
    if ideograph { u32::MAX } else { u32::from(c) >> 8 }
}

/// The fonts of the family that `name` names, if the system has it, the
/// weight closest to the name's first, with their family name.
pub(crate) fn named_fonts(finder: &mut Finder, name: &str, parsed: &FontName<'_>) -> Vec<(String, Source)> {
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
    let mut candidates = drawn(finder.candidates(&query), ' ', parsed.weight);
    candidates.retain(|(family, ..)| family.eq_ignore_ascii_case(parsed.family));
    nearest(candidates, parsed.weight)
}

/// The fonts the platform picks for `c` in the family, weight and style of
/// `first`, the font stack's first name, the weight closest to it first,
/// with their family names.
pub(crate) fn picked_fonts(finder: &mut Finder, first: &FontName<'_>, c: char) -> Vec<(String, Source)> {
    let languages = if class(c) == u32::MAX { languages::for_ideographs(&system().preferred) } else { Vec::new() };
    let query = Query {
        name: None,
        family: Some(first.family).filter(|family| !family.is_empty()),
        probe: c,
        languages: &languages,
        weight: first.weight,
        italic: first.italic,
    };
    nearest(drawn(finder.candidates(&query), c, first.weight), first.weight)
}

/// How many of a lookup's candidates to read.
pub(crate) const MAX_CANDIDATES: usize = 16;

/// The first of `candidates` that have `c`, with the weight they draw at
/// when asked for `weight`.
fn drawn(candidates: Vec<(String, Source)>, c: char, weight: u16) -> Vec<(String, Source, u16)> {
    candidates
        .into_iter()
        .take(MAX_CANDIDATES)
        .filter_map(|(family, source)| {
            let drawn = source.drawn_weight(c, weight)?;
            Some((family, source, drawn))
        })
        .collect()
}

/// `candidates`, those that draw closest to `weight` first, otherwise in
/// order.
fn nearest(mut candidates: Vec<(String, Source, u16)>, weight: u16) -> Vec<(String, Source)> {
    candidates.sort_by_key(|(_, _, drawn)| drawn.abs_diff(weight));
    candidates.into_iter().map(|(family, source, _)| (family, source)).collect()
}

#[cfg(test)]
mod tests {
    // Which fonts the system has depends on the machine, so the tests check
    // the fonts it finds, and pass with none; `--nocapture` lists them.

    use std::num::NonZeroU16;

    use super::{SystemFonts, class, first_with, named_fonts, picked_fonts};
    use crate::text::glyph::{Glyph, GlyphIDType};
    use crate::text::local_fonts::FontChooser;
    use crate::text::system_fonts::{Finder, system};
    use crate::text::{Shaper, font_name};

    fn stack(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// What `f` finds with the platform's font matcher: `None` without one.
    fn with_finder<R>(f: impl FnOnce(&mut Finder) -> R) -> Option<R> {
        Some(f(&mut system().fonts.as_ref()?.lock().finder))
    }

    /// The glyph that the face chosen for `c` in `font_stack` maps it to.
    fn chosen_glyph(font_stack: &[&str], c: char) -> Option<Glyph> {
        let face = SystemFonts.choose(&stack(font_stack), &[c])?;
        let data = face.data.expect("a device face holds its file");
        let shaper = Shaper::at_weight(GlyphIDType::face(NonZeroU16::MIN), data, f32::from(face.weight));
        let id = shaper.glyph_of(c).unwrap_or_else(|| panic!("no glyph for {c} in {}", face.url));
        Some(shaper.rasterize_glyph(id))
    }

    #[test]
    fn a_picked_font_has_the_character() {
        for c in ['a', 'ä', 'Ж', 'α', 'א', 'ب', '中', 'あ', '한', 'ሀ'] {
            for name in ["", "Open Sans Regular", "Noto Sans Bold"] {
                let found = with_finder(|finder| picked_fonts(finder, &font_name::parse(name), c).into_iter().next());
                let found = found.flatten();
                println!("{c} for {name:?}: {found:?}");
                if let Some((_, source)) = found {
                    assert!(source.drawn_weight(c, 400).is_some(), "{c} for {name:?}");
                }
            }
        }
    }

    #[test]
    fn a_name_gets_its_family_if_the_system_has_it() {
        with_finder(|finder| {
            // The family the platform picks for 'a' is one the system has.
            let Some((family, _)) = picked_fonts(finder, &font_name::parse(""), 'a').into_iter().next() else {
                return;
            };
            let name = format!("{family} Bold");
            let found = named_fonts(finder, &name, &font_name::parse(&name));
            println!("{name}: {:?}", found.first());
            assert_ne!(found.len(), 0);
            assert!(found.iter().all(|(found, _)| found.eq_ignore_ascii_case(&family)));
            let missing = "No Such Font Regular";
            assert_eq!(named_fonts(finder, missing, &font_name::parse(missing)).len(), 0);
        });
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
    fn a_chosen_face_draws_its_character() {
        for c in ['a', '中', '한'] {
            let glyph = chosen_glyph(&["Open Sans Regular"], c);
            println!("{c}: {:?}", glyph.as_ref().map(|glyph| glyph.metrics));
            if let Some(glyph) = glyph {
                assert!(glyph.metrics.advance > 0);
                assert!(glyph.bitmap.data.iter().any(|&alpha| alpha > 0));
            }
        }
    }

    // A variable font draws at each name's weight, a static family from its
    // file of that weight.
    #[test]
    fn a_bold_name_draws_bolder_where_the_system_has_a_bolder_font() {
        for c in ['a', 'Ж', 'α', 'א', 'ب'] {
            let picked = with_finder(|finder| picked_fonts(finder, &font_name::parse(""), c).into_iter().next());
            let Some((family, _)) = picked.flatten() else { continue };
            let ink = |name: &str| {
                let glyph = chosen_glyph(&[name], c)?;
                Some(glyph.bitmap.data.iter().map(|&alpha| u64::from(alpha)).sum::<u64>())
            };
            let drawn = |name: &str| {
                let found = with_finder(|finder| named_fonts(finder, name, &font_name::parse(name)).into_iter().next());
                let (_, source) = found.flatten()?;
                Some((source.drawn_weight(c, font_name::parse(name).weight)?, source))
            };
            let (regular, bold) = (format!("{family} Regular"), format!("{family} Bold"));
            let (Some((regular_weight, _)), Some((bold_weight, source))) = (drawn(&regular), drawn(&bold)) else {
                continue;
            };
            println!("{c}: {regular_weight} {:?}, {bold_weight} {:?} from {source:?}", ink(&regular), ink(&bold));
            if bold_weight > regular_weight {
                assert!(ink(&bold) > ink(&regular), "{c} in {family}");
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
