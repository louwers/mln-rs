//! The system's fonts, for the text that a style's own fonts leave to them:
//!
//! - [`font_face`]: the system's fonts as a map's fallback face
//!   (`FontFace::system`), which the map shapes text with: in a style with
//!   glyph PBFs, the scripts they can't shape (the Indic scripts, Sinhala,
//!   Tibetan, Myanmar and Khmer) that the system has fonts for; in a style
//!   without, all the text the style's font faces don't draw.
//! - [`SystemFonts`]: the `FontChooser` that chooses the font of each
//!   character of a label, for that shaping, and for the glyph of any other
//!   character, one at a time: a glyph a style's PBFs lack, or a CJK
//!   character. It is the first font that the label's font stack names that
//!   the system has and that has the character, else the font the system
//!   picks for it, at the weight and style that the stack's names imply. The
//!   chosen font is a `FontFace::device` that holds its file, which the map
//!   loads as it loads a style's font faces.
//!
//! It asks the platform's font matcher, for the user's preferred languages:
//! fontconfig on Linux and the BSDs, `AFontMatcher` on Android 10 and later,
//! CoreText on Apple's platforms and DirectWrite on Windows. Without one,
//! such as on older Android, it finds no fonts. The browser build finds none
//! either: the browser draws local glyphs itself (`BrowserGlyphRasterizer`).
//!
//! The matcher loads once per process, on a thread of its own
//! ([`prefetch`]), and each font the first time a map needs it, off the
//! render thread. A font's file is memory-mapped when it is first chosen,
//! and a CoreText font's tables are copied into a file then.

use std::sync::OnceLock;

use crate::text::glyph::{FontFace, FontFaceRange};

#[cfg(not(target_family = "wasm"))]
mod glyphs;
#[cfg(not(target_family = "wasm"))]
mod languages;
#[cfg(not(target_family = "wasm"))]
mod scripts;
#[cfg(not(target_family = "wasm"))]
mod sfnt;
#[cfg(not(target_family = "wasm"))]
mod source;

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_vendor = "apple")]
mod apple;
#[cfg(windows)]
mod directwrite;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod fontconfig;

#[cfg(not(target_family = "wasm"))]
pub use glyphs::SystemFonts;

/// The ranges of [`shaped_ranges`], once found.
static SHAPED: OnceLock<Vec<FontFaceRange>> = OnceLock::new();

/// Starts loading the platform's font matcher and finding
/// [`shaped_ranges`] on a thread of their own, unless that has started, so
/// that the first map that needs them waits less.
pub fn prefetch() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if SHAPED.get().is_some() {
        return;
    }
    STARTED.get_or_init(|| {
        // Without a thread, the first `shaped_ranges` finds them.
        let _ = mln_util::thread::builder("system fonts").spawn(shaped_ranges);
    });
}

/// The code points of the scripts that glyph PBFs can't shape that the
/// system has a font for. Finds them the first time, which can take a
/// moment; then it returns the same ranges.
pub fn shaped_ranges() -> &'static [FontFaceRange] {
    SHAPED.get_or_init(find)
}

/// The system's fonts as a map's fallback face, for [`shaped_ranges`]
/// where the style has glyph PBFs (`Map::set_fallback_font_face_list`):
/// `None` without a font matcher, as in the browser.
#[must_use]
pub fn font_face() -> Option<FontFace> {
    #[cfg(target_family = "wasm")]
    return None;
    #[cfg(not(target_family = "wasm"))]
    system().fonts.is_some().then(|| FontFace::system(shaped_ranges().to_vec()))
}

#[cfg(target_family = "wasm")]
fn find() -> Vec<FontFaceRange> {
    Vec::new()
}

#[cfg(not(target_family = "wasm"))]
fn find() -> Vec<FontFaceRange> {
    let Some(fonts) = &system().fonts else { return Vec::new() };
    let mut fonts = fonts.lock();
    let ranges = find_with(&mut fonts.finder);
    #[cfg(target_os = "android")]
    {
        let Finder::Android(matcher) = &fonts.finder;
        matcher.prepare();
    }
    ranges
}

/// The ranges of the scripts that `finder` has a font for.
#[cfg(not(target_family = "wasm"))]
fn find_with(finder: &mut Finder) -> Vec<FontFaceRange> {
    let mut ranges = Vec::new();
    for script in scripts::SCRIPTS {
        let languages = [script.language.to_owned()];
        let query =
            Query { name: None, family: None, probe: script.probe, languages: &languages, weight: 400, italic: false };
        let has_font = finder
            .candidates(&query)
            .iter()
            .take(glyphs::MAX_CANDIDATES)
            .any(|(_, source)| source.drawn_weight(script.probe, 400).is_some());
        if has_font {
            ranges.extend_from_slice(script.ranges);
        }
    }
    ranges
}

/// What to look a font up for.
#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Query<'a> {
    /// The font's full name, as a `text-font` names it: "Open Sans Bold".
    /// CoreText finds a font by it; the other matchers by the family.
    #[cfg_attr(not(target_vendor = "apple"), expect(dead_code, reason = "only CoreText finds fonts by full name"))]
    pub(crate) name: Option<&'a str>,
    /// The family to prefer, "Open Sans", or `None` for the system's sans
    /// serif.
    pub(crate) family: Option<&'a str>,
    /// A character the font has to have.
    pub(crate) probe: char,
    /// The languages of the text, most preferred first, which choose between
    /// the fonts of Chinese, Japanese and Korean. Empty to leave the choice to
    /// the platform.
    pub(crate) languages: &'a [String],
    /// The OpenType weight to prefer.
    pub(crate) weight: u16,
    pub(crate) italic: bool,
}

/// The process's fonts, and the user's languages.
#[cfg(not(target_family = "wasm"))]
pub(crate) struct System {
    /// The platform's font matcher and the fonts found with it, if it has
    /// one. One lock, so that a lookup and what it finds stay together.
    pub(crate) fonts: Option<parking_lot::Mutex<glyphs::Fonts>>,
    /// The user's preferred languages, most preferred first.
    pub(crate) preferred: Vec<String>,
}

/// The system's fonts, loaded the first time.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn system() -> &'static System {
    static SYSTEM: OnceLock<System> = OnceLock::new();
    SYSTEM.get_or_init(|| System {
        fonts: Finder::new().map(|finder| parking_lot::Mutex::new(glyphs::Fonts::new(finder))),
        preferred: languages::preferred(),
    })
}

/// The platform's way to find fonts.
#[cfg(not(target_family = "wasm"))]
pub(crate) enum Finder {
    #[cfg(target_vendor = "apple")]
    CoreText(apple::CoreText),
    #[cfg(windows)]
    DirectWrite(directwrite::DirectWrite),
    #[cfg(target_os = "android")]
    Android(android::FontMatcher),
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    Fontconfig(fontconfig::Fontconfig),
}

#[cfg(not(target_family = "wasm"))]
impl Finder {
    /// The platform's font matcher, or `None` where it has none.
    fn new() -> Option<Finder> {
        #[cfg(target_vendor = "apple")]
        return Some(Finder::CoreText(apple::CoreText::default()));
        #[cfg(windows)]
        return directwrite::DirectWrite::new().map(Finder::DirectWrite);
        #[cfg(target_os = "android")]
        return android::FontMatcher::new().map(Finder::Android);
        #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
        return fontconfig::Fontconfig::new().map(Finder::Fontconfig);
        #[expect(unreachable_code, reason = "a platform without a matcher reaches it")]
        None
    }

    /// The fonts that may draw `query`'s character, best first, with their
    /// family names. Some may lack it.
    pub(crate) fn candidates(&mut self, query: &Query<'_>) -> Vec<(String, source::Source)> {
        match self {
            #[cfg(target_vendor = "apple")]
            Finder::CoreText(core_text) => core_text.candidates(query),
            #[cfg(windows)]
            Finder::DirectWrite(fallback) => fallback.candidates(query),
            #[cfg(target_os = "android")]
            Finder::Android(matcher) => matcher.candidates(query),
            #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
            Finder::Fontconfig(fontconfig) => fontconfig.candidates(query),
        }
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    // Which fonts it finds depends on the machine, so the tests check the
    // fonts it finds, and pass with none; `--nocapture` lists them.

    use skrifa::MetadataProvider;

    use super::{SystemFonts, scripts, shaped_ranges};
    use crate::text::local_fonts::FontChooser;
    use crate::text::{ShapedText, Shaper};

    /// Text of the script of `probe` that its font's shaping rules change:
    /// a conjunct, a split or reordered vowel, or a stack.
    fn shaped_sample(probe: char) -> &'static str {
        match probe {
            '\u{0915}' => "क्ष",
            '\u{0995}' => "ক্ষ",
            '\u{0A15}' => "ਕ੍ਰ",
            '\u{0A95}' => "ક્ષ",
            '\u{0B15}' => "କ୍ଷ",
            '\u{0B95}' => "கொ",
            '\u{0C15}' => "క్క",
            '\u{0C95}' => "ಕ್ಕ",
            '\u{0D15}' => "ക്ക",
            '\u{0D9A}' => "කො",
            '\u{0F40}' => "སྒྲ",
            '\u{1000}' => "ကြ",
            '\u{1780}' => "ក្ក",
            _ => panic!("no sample for {probe:?}"),
        }
    }

    /// For each script the system has fonts for, the font a label gets for
    /// it shapes it: its rules substitute, reorder or position the glyphs of
    /// a sample, which would otherwise come out one after another as the
    /// font maps them.
    #[test]
    fn the_fonts_of_the_shaped_scripts_shape_them() {
        let started = std::time::Instant::now();
        let ranges = shaped_ranges();
        println!("found fonts for {ranges:04X?} in {:?}", started.elapsed());
        for script in scripts::SCRIPTS.iter().filter(|script| script.ranges.iter().all(|r| ranges.contains(r))) {
            let sample = shaped_sample(script.probe);
            let face = SystemFonts.choose(&Vec::new(), &[script.probe]).expect("a font for a shaped script");
            let data = face.data.clone().expect("a device face holds its file");
            let font = skrifa::FontRef::from_index(data.as_bytes(), data.face_index()).unwrap();
            let nominal: Vec<u16> =
                sample.chars().map(|c| font.charmap().map(c).map_or(0, |glyph| glyph.to_u32() as u16)).collect();
            let shaper = Shaper::at_weight(face.type_, data, f32::from(face.weight));
            let mut shaped = ShapedText::default();
            shaper.shape(&sample.chars().collect::<Vec<_>>(), false, &mut shaped);
            let glyphs: Vec<u16> = shaped.glyphs.iter().map(|id| id.code).collect();
            let moved = shaped.adjusts.iter().any(|adjust| adjust.x_offset != 0.0 || adjust.y_offset != 0.0);
            println!("{}: {sample} {nominal:?} -> {glyphs:?}{}", face.url, if moved { " moved" } else { "" });
            assert!(glyphs != nominal || moved, "{} doesn't shape {sample}", face.url);
        }
    }

    #[test]
    fn a_font_has_the_character_it_is_for() {
        for c in ['a', 'ب', '\u{0915}', '中'] {
            let Some(face) = SystemFonts.choose(&vec!["Open Sans Bold".to_owned()], &[c]) else { continue };
            let data = face.data.expect("a device face holds its file");
            let font = skrifa::FontRef::from_index(data.as_bytes(), data.face_index()).unwrap();
            println!("{c}: {} at {}", face.url, face.weight);
            assert!(font.charmap().map(c).is_some(), "{c} in {}", face.url);
            assert_eq!(face.weight, 700);
        }
    }
}
