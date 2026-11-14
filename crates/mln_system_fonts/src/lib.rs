//! The system's fonts for the scripts that a style's fonts and glyph PBFs
//! can't draw well: the scripts that need complex shaping (the Indic
//! scripts, Sinhala, Tibetan, Myanmar and Khmer), and CJK, whose glyphs
//! would otherwise download a PBF for every few characters.
//!
//! For each script, it picks one installed font that has the script's
//! glyphs and draws near regular weight (`pick`), looked up for the user's preferred
//! languages, and gives the map that font for the script's code points only.
//! It asks the platform's font matcher where it has one: fontconfig on Linux
//! and the BSDs, `AFontMatcher` on Android 10 and later. Elsewhere, or
//! without it, it scans the system's font files
//! for families known to draw each script. The browser build finds none: the
//! browser draws CJK glyphs itself (`DefaultLocalGlyphRasterizer`).
//!
//! The fonts are found once per process, on a thread of their own. A font's
//! file is memory-mapped the first time the map needs it.

use std::sync::OnceLock;

use mln_text::glyph::{FontFace, FontFaces};

#[cfg(not(target_family = "wasm"))]
mod languages;
#[cfg(not(target_family = "wasm"))]
mod scan;
#[cfg(not(target_family = "wasm"))]
mod scripts;
#[cfg(not(target_family = "wasm"))]
mod sfnt;
#[cfg(not(target_family = "wasm"))]
mod source;

#[cfg(target_os = "android")]
pub mod android;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod fontconfig;

/// A system font chosen as a fallback.
#[derive(Clone, Debug)]
pub struct SystemFontFace {
    /// Its family name.
    pub family: String,
    pub face: FontFace,
}

/// The fallback fonts, once found.
static FACES: OnceLock<Vec<SystemFontFace>> = OnceLock::new();

/// Starts finding the fallback fonts on a thread of their own, unless that
/// has started, so that [`fallback_font_faces`] waits less.
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

/// The fallback fonts for the scripts that need them, from the fonts
/// installed on this system: at most one per script. Finds them the first
/// time, which can take a moment; then it returns the same fonts.
pub fn fallback_font_faces() -> &'static [SystemFontFace] {
    FACES.get_or_init(find)
}

/// The faces of [`fallback_font_faces`] alone, for
/// `Style::set_fallback_font_face_list`.
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
    find_with(&Finder::new(), &languages::preferred())
}

/// The fallback fonts that `finder` finds for the user's `preferred`
/// languages.
#[cfg(not(target_family = "wasm"))]
fn find_with(finder: &Finder, preferred: &[String]) -> Vec<SystemFontFace> {
    let mut files = source::Files::default();
    let mut faces = Vec::new();
    for script in scripts::SCRIPTS {
        let languages = languages::for_script(script, preferred);
        let Some((family, source)) = pick(script, finder.candidates(script, &languages)) else {
            continue;
        };
        let url = source.url();
        let bytes = files.data(&source);
        faces.push(SystemFontFace { family, face: FontFace::fallback(&url, script.ranges.to_vec()).with_data(bytes) });
    }
    faces
}

/// The best of `candidates` for `script`: the first that has its glyphs and
/// draws near regular weight. Glyph PBFs can't shape the scripts that need
/// it, so for those, any weight beats none, and the first that has their
/// glyphs is the last resort; CJK falls back to the PBFs instead.
#[cfg(not(target_family = "wasm"))]
fn pick(script: &scripts::Script, candidates: Vec<(String, source::Source)>) -> Option<(String, source::Source)> {
    let mut last_resort = None;
    for (family, source) in candidates {
        let Some(weight) = source.drawn_weight(script.probe) else { continue };
        if source::near_regular(weight) {
            return Some((family, source));
        }
        if script.needs_shaping && last_resort.is_none() {
            last_resort = Some((family, source));
        }
    }
    last_resort
}

/// The platform's way to find fonts.
#[cfg(not(target_family = "wasm"))]
enum Finder {
    #[cfg(target_os = "android")]
    Android(android::FontMatcher),
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    Fontconfig(fontconfig::Fontconfig),
    Scan(scan::Scan),
}

#[cfg(not(target_family = "wasm"))]
impl Finder {
    fn new() -> Finder {
        #[cfg(target_os = "android")]
        if let Some(matcher) = android::FontMatcher::new() {
            return Finder::Android(matcher);
        }
        #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
        if let Some(fontconfig) = fontconfig::Fontconfig::new() {
            return Finder::Fontconfig(fontconfig);
        }
        Finder::Scan(scan::Scan::new())
    }

    /// The fonts that may draw `script` for `languages`, best first, with
    /// their family names.
    fn candidates(&self, script: &scripts::Script, languages: &[String]) -> Vec<(String, source::Source)> {
        match self {
            #[cfg(target_os = "android")]
            Finder::Android(matcher) => matcher.candidates(script, languages),
            #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
            Finder::Fontconfig(fontconfig) => fontconfig.candidates(script, languages),
            Finder::Scan(scan) => {
                let _ = languages;
                scan.candidates(script)
            }
        }
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    // Which fonts it finds depends on the machine, so the tests check each
    // font it chose, and pass with none; `--nocapture` lists them.

    use mln_text::glyph::GlyphIDType;
    use skrifa::MetadataProvider;

    use super::{Finder, SystemFontFace, fallback_font_faces, find_with, scan};

    fn check(faces: &[SystemFontFace]) {
        for face in faces {
            let (family, face) = (&face.family, &face.face);
            let bytes = face.data.as_ref().expect("a system face holds its font file").as_bytes();
            println!("{family}: {} ({} bytes) for {:04X?}", face.url, bytes.len(), face.ranges);
            assert_eq!(face.name, "");
            assert!(face.url.starts_with("file://"));
            assert_ne!(face.type_, GlyphIDType::FontPBF);
            let font = skrifa::FontRef::from_index(bytes, face.data.as_ref().unwrap().face_index()).expect("a font");
            let covered =
                font.charmap().mappings().any(|(c, _)| face.ranges.iter().any(|&(lo, hi)| c >= lo && c <= hi));
            assert!(covered, "{family} has no glyph in its ranges");
        }
    }

    #[test]
    fn chosen_faces_load_and_cover_their_ranges() {
        let started = std::time::Instant::now();
        let faces = fallback_font_faces();
        println!("found {} fallback faces in {:?}", faces.len(), started.elapsed());
        check(faces);
    }

    #[test]
    fn faces_from_the_scan_load_and_cover_their_ranges() {
        let faces = find_with(&Finder::Scan(scan::Scan::new()), &["ko-KR".to_owned()]);
        println!("the scan found {} fallback faces", faces.len());
        check(&faces);
    }
}
