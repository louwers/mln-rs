//! The system's fonts, for the text that a style's own fonts leave to them:
//!
//! - [`fallback_font_faces`]: a font for each script that glyph PBFs can't
//!   shape (the Indic scripts, Sinhala, Tibetan, Myanmar and Khmer), which
//!   the map shapes with HarfBuzz. For each script, it picks one installed
//!   font that has the script's glyphs and draws near regular weight
//!   (`pick`).
//! - [`SystemGlyphRasterizer`]: the glyph of any other character, one at a
//!   time, for a style without glyph PBFs, a glyph its PBFs lack, or a CJK
//!   character. It draws from the font that the label's font stack names, if
//!   the system has it, else from the font the system picks for the
//!   character, at the weight and style that the stack's names imply.
//!
//! It asks the platform's font matcher, for the user's preferred languages:
//! fontconfig on Linux and the BSDs, `AFontMatcher` on Android 10 and later,
//! CoreText on Apple's platforms and DirectWrite on Windows. Without one,
//! such as on older Android, it finds no fonts. The browser build finds none
//! either: the browser draws local glyphs itself (`BrowserGlyphRasterizer`).
//!
//! The matcher loads once per process, on a thread of its own
//! ([`prefetch`]), and each font the first time a map needs it. A font's
//! file is memory-mapped the first time the map draws from it.

use std::sync::OnceLock;

use mln_text::glyph::{FontFace, FontFaces};

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
pub use glyphs::SystemGlyphRasterizer;

/// A system font chosen as a fallback.
#[derive(Clone, Debug)]
pub struct SystemFontFace {
    /// Its family name.
    pub family: String,
    pub face: FontFace,
}

/// The fallback fonts, once found.
static FACES: OnceLock<Vec<SystemFontFace>> = OnceLock::new();

/// Starts loading the platform's font matcher and finding the fallback
/// fonts on a thread of their own, unless that has started, so that the
/// first map that needs them waits less.
pub fn prefetch() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if FACES.get().is_some() {
        return;
    }
    STARTED.get_or_init(|| {
        // Without a thread, the first `fallback_font_faces` finds them.
        let _ = mln_util::thread::builder("system fonts").spawn(fallback_font_faces);
    });
}

/// The fallback fonts for the scripts that glyph PBFs can't shape, from the
/// fonts installed on this system: at most one per script. Finds them the
/// first time, which can take a moment; then it returns the same fonts.
pub fn fallback_font_faces() -> &'static [SystemFontFace] {
    FACES.get_or_init(find)
}

/// The faces of [`fallback_font_faces`] alone, for
/// `Map::set_fallback_font_face_list`.
#[must_use]
pub fn fallback_font_face_list() -> FontFaces {
    fallback_font_faces().iter().map(|face| face.face.clone()).collect()
}

#[cfg(target_family = "wasm")]
fn find() -> Vec<SystemFontFace> {
    Vec::new()
}

#[cfg(not(target_family = "wasm"))]
fn find() -> Vec<SystemFontFace> {
    let Some(finder) = &system().finder else { return Vec::new() };
    let finder = finder.lock();
    let faces = find_with(&finder);
    #[cfg(target_os = "android")]
    {
        let Finder::Android(matcher) = &*finder;
        matcher.prepare();
    }
    faces
}

/// The fallback fonts that `finder` finds.
#[cfg(not(target_family = "wasm"))]
fn find_with(finder: &Finder) -> Vec<SystemFontFace> {
    let mut files = source::Files::default();
    let mut faces = Vec::new();
    for script in scripts::SCRIPTS {
        let languages = [script.language.to_owned()];
        let query =
            Query { name: None, family: None, probe: script.probe, languages: &languages, weight: 400, italic: false };
        let Some((family, source)) = pick(script, finder.candidates(&query)) else {
            continue;
        };
        let url = source.url();
        let bytes = files.data(&source);
        faces.push(SystemFontFace { family, face: FontFace::fallback(&url, script.ranges.to_vec()).with_data(bytes) });
    }
    faces
}

/// The best of `candidates` for `script`: the first that has its glyphs and
/// draws near regular weight, else the first that has its glyphs. Glyph PBFs
/// can't shape the script, so any weight beats none.
#[cfg(not(target_family = "wasm"))]
fn pick(script: &scripts::Script, candidates: Vec<(String, source::Source)>) -> Option<(String, source::Source)> {
    let mut last_resort = None;
    for (family, source) in candidates {
        let Some(weight) = source.drawn_weight(script.probe, 400) else { continue };
        if source::near_regular(weight) {
            return Some((family, source));
        }
        if last_resort.is_none() {
            last_resort = Some((family, source));
        }
    }
    last_resort
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

/// The process's way to find fonts, and the user's languages.
#[cfg(not(target_family = "wasm"))]
pub(crate) struct System {
    /// The platform's font matcher, if it has one.
    finder: Option<parking_lot::Mutex<Finder>>,
    /// The user's preferred languages, most preferred first.
    pub(crate) preferred: Vec<String>,
}

#[cfg(not(target_family = "wasm"))]
impl System {
    /// The fonts that may draw `query`'s character, best first: none
    /// without a font matcher.
    pub(crate) fn candidates(&self, query: &Query<'_>) -> Vec<(String, source::Source)> {
        self.finder.as_ref().map(|finder| finder.lock().candidates(query)).unwrap_or_default()
    }
}

/// The system's fonts, loaded the first time.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn system() -> &'static System {
    static SYSTEM: OnceLock<System> = OnceLock::new();
    SYSTEM.get_or_init(|| System {
        finder: Finder::new().map(parking_lot::Mutex::new),
        preferred: languages::preferred(),
    })
}

/// The platform's way to find fonts.
#[cfg(not(target_family = "wasm"))]
pub(crate) enum Finder {
    #[cfg(target_vendor = "apple")]
    CoreText,
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
        return Some(Finder::CoreText);
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
    pub(crate) fn candidates(&self, query: &Query<'_>) -> Vec<(String, source::Source)> {
        match self {
            #[cfg(target_vendor = "apple")]
            Finder::CoreText => apple::candidates(query),
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
    // Which fonts it finds depends on the machine, so the tests check each
    // font it chose, and pass with none; `--nocapture` lists them.

    use mln_text::glyph::GlyphIDType;
    use mln_text::{FreeTypeLibrary, HBShaper};
    use skrifa::MetadataProvider;

    use super::{SystemFontFace, fallback_font_faces, scripts};

    /// Text of the script of `probe` that its font's shaping rules change:
    /// a conjunct, a split or reordered vowel, or a stack.
    fn shaped_sample(probe: char) -> Option<&'static str> {
        Some(match probe {
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
            _ => return None,
        })
    }

    /// Each font for a script that needs shaping shapes it: its rules
    /// substitute, reorder or position the glyphs of a sample, which the
    /// map's shaper would otherwise draw one after another as the font maps
    /// them.
    fn check_shaping(faces: &[SystemFontFace]) {
        for script in scripts::SCRIPTS {
            let Some(face) = faces.iter().find(|face| face.face.ranges == script.ranges) else { continue };
            let sample = shaped_sample(script.probe).expect("a sample for each script that needs shaping");
            let data = face.face.data.clone().unwrap();
            let font = skrifa::FontRef::from_index(data.as_bytes(), data.face_index()).unwrap();
            let nominal: Vec<u16> =
                sample.chars().map(|c| font.charmap().map(c).map_or(0, |glyph| glyph.to_u32() as u16)).collect();
            let shaper = HBShaper::new(face.face.type_, data, &FreeTypeLibrary::new());
            let text: Vec<char> = sample.chars().collect();
            let (mut ids, mut adjusts) = (Vec::new(), Vec::new());
            shaper.create_complex_glyph_ids(&text, false, &mut ids, &mut adjusts);
            let shaped: Vec<u16> = ids.iter().map(|id| id.code).collect();
            let moved = adjusts.iter().any(|adjust| adjust.x_offset != 0.0 || adjust.y_offset != 0.0);
            println!("{}: {sample} {nominal:?} -> {shaped:?}{}", face.family, if moved { " moved" } else { "" });
            assert!(shaped != nominal || moved, "{} doesn't shape {sample}", face.family);
        }
    }

    fn check(faces: &[SystemFontFace]) {
        for face in faces {
            let (family, face) = (&face.family, &face.face);
            let bytes = face.data.as_ref().expect("a system face holds its font file").as_bytes();
            println!("{family}: {} ({} bytes) for {:04X?}", face.url, bytes.len(), face.ranges);
            assert_eq!(face.name, "");
            assert!(face.url.starts_with("file://") || face.url.starts_with("font://"));
            assert_ne!(face.type_, GlyphIDType::FontPBF);
            let font = skrifa::FontRef::from_index(bytes, face.data.as_ref().unwrap().face_index()).expect("a font");
            let covered =
                font.charmap().mappings().any(|(c, _)| face.ranges.iter().any(|&(lo, hi)| c >= lo && c <= hi));
            assert!(covered, "{family} has no glyph in its ranges");
            assert!(
                scripts::SCRIPTS.iter().any(|script| script.ranges == face.ranges),
                "{family} is for a script that needs shaping"
            );
        }
    }

    #[test]
    fn chosen_faces_load_and_cover_their_ranges() {
        let started = std::time::Instant::now();
        let faces = fallback_font_faces();
        println!("found {} fallback faces in {:?}", faces.len(), started.elapsed());
        check(faces);
        check_shaping(faces);
    }
}
