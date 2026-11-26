//! The fonts for a character from Android's font API (API level 29): the
//! system's fonts of the family asked for, by the family names in their
//! files (`ASystemFontIterator`), then the font that the platform would draw
//! the text in (`AFontMatcher`). The matcher knows only the families that the
//! system's font configuration names, such as `serif`, and falls back to
//! `sans-serif` for any other. The functions are looked up at run time, so
//! that an app that runs on older versions finds no system fonts. And the
//! app's languages, from Java, through [`init`].
//!
//! The matcher draws on the system font collection that an app process
//! inherits; in a process that isn't an app, such as a test binary run from
//! `adb shell`, it has none and crashes. So it is used only once [`init`]
//! ran, which every app that uses the library calls.

#![expect(unsafe_code, reason = "the NDK's font API and JNI are the FFI here")]

use std::ffi::{CStr, CString, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::OnceLock;

use jni::objects::JString;
use jni::{EnvUnowned, Outcome, jni_sig, jni_str};
use libloading::Library;

use crate::text::system_fonts::Query;
use crate::text::system_fonts::source::Source;

/// The app's languages, most preferred first, once [`init`] ran: empty
/// when it couldn't read them.
static LANGUAGES: OnceLock<Vec<String>> = OnceLock::new();

/// Reads the app's languages (`LocaleList.getDefault()`), which the system
/// fonts are looked up for: native code can read only the device's first
/// language. Call it before the fonts are found, from a thread attached to
/// the app's Java VM; a later call changes nothing. Leaves no Java exception
/// pending.
///
/// # Safety
///
/// `jni_env` is the calling thread's `JNIEnv*`.
pub unsafe fn init(jni_env: *mut c_void) {
    // SAFETY: the caller's contract; the local references that JNI makes
    // here go with the caller's frame.
    let mut env = unsafe { EnvUnowned::from_raw(jni_env.cast()) };
    let outcome = env
        .with_env(|env| -> Result<String, jni::errors::Error> {
            let read = (|| {
                let list = env
                    .call_static_method(
                        jni_str!("android/os/LocaleList"),
                        jni_str!("getDefault"),
                        jni_sig!("()Landroid/os/LocaleList;"),
                        &[],
                    )?
                    .l()?;
                let tags =
                    env.call_method(&list, jni_str!("toLanguageTags"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
                env.cast_local::<JString>(tags)?.try_to_string(env)
            })();
            if env.exception_check() {
                env.exception_clear();
            }
            read
        })
        .into_outcome();
    let languages = match outcome {
        Outcome::Ok(tags) => tags.split(',').filter(|tag| !tag.is_empty()).map(str::to_owned).collect(),
        Outcome::Err(_) | Outcome::Panic(_) => Vec::new(),
    };
    let _ = LANGUAGES.set(languages);
}

/// The app's languages, once [`init`] read them.
pub(crate) fn languages() -> Option<Vec<String>> {
    LANGUAGES.get().filter(|languages| !languages.is_empty()).cloned()
}

/// A system font, with the family its file names.
struct SystemFont {
    family: String,
    weight: u16,
    italic: bool,
    source: Source,
}

/// The functions of the NDK's `font_matcher.h`, `font.h` and
/// `system_fonts.h` that this uses, and the system's fonts.
pub(crate) struct FontMatcher {
    create: unsafe extern "C" fn() -> *mut c_void,
    destroy: unsafe extern "C" fn(*mut c_void),
    set_locales: unsafe extern "C" fn(*mut c_void, *const c_char),
    set_style: unsafe extern "C" fn(*mut c_void, u16, bool),
    matches: unsafe extern "C" fn(*const c_void, *const c_char, *const u16, u32, *mut u32) -> *mut c_void,
    close: unsafe extern "C" fn(*mut c_void),
    file_path: unsafe extern "C" fn(*const c_void) -> *const c_char,
    collection_index: unsafe extern "C" fn(*const c_void) -> usize,
    font_weight: unsafe extern "C" fn(*const c_void) -> u16,
    font_is_italic: unsafe extern "C" fn(*const c_void) -> bool,
    iterator_open: unsafe extern "C" fn() -> *mut c_void,
    iterator_next: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    iterator_close: unsafe extern "C" fn(*mut c_void),
    /// The system's fonts, listed the first time a font is looked up by
    /// family.
    fonts: OnceLock<Vec<SystemFont>>,
    // Last, so that the functions above stay loaded while they are used.
    _library: Library,
}

impl FontMatcher {
    /// The font matcher in an app, once [`init`] ran: `None` before API level
    /// 29, or outside an app.
    pub(crate) fn new() -> Option<FontMatcher> {
        LANGUAGES.get()?;
        Self::load()
    }

    /// The matcher's functions, or `None` before API level 29.
    fn load() -> Option<FontMatcher> {
        // SAFETY: every app has libandroid loaded already.
        let library = unsafe { Library::new("libandroid.so") }.ok()?;
        macro_rules! symbol {
            ($name:literal) => {
                // SAFETY: the signature is that of the function in the NDK's
                // header.
                *unsafe { library.get(concat!($name, "\0").as_bytes()) }.ok()?
            };
        }
        Some(FontMatcher {
            create: symbol!("AFontMatcher_create"),
            destroy: symbol!("AFontMatcher_destroy"),
            set_locales: symbol!("AFontMatcher_setLocales"),
            set_style: symbol!("AFontMatcher_setStyle"),
            matches: symbol!("AFontMatcher_match"),
            close: symbol!("AFont_close"),
            file_path: symbol!("AFont_getFontFilePath"),
            collection_index: symbol!("AFont_getCollectionIndex"),
            font_weight: symbol!("AFont_getWeight"),
            font_is_italic: symbol!("AFont_isItalic"),
            iterator_open: symbol!("ASystemFontIterator_open"),
            iterator_next: symbol!("ASystemFontIterator_next"),
            iterator_close: symbol!("ASystemFontIterator_close"),
            fonts: OnceLock::new(),
            _library: library,
        })
    }

    /// Lists the system's fonts, ahead of the first lookup by family.
    pub(crate) fn prepare(&self) {
        self.fonts();
    }

    /// The fonts that may draw `query`'s character, with their family names:
    /// the system's fonts of its family, the style and weight closest to
    /// those asked for first, then the font that Android draws the character
    /// in. They may lack it: the matcher returns a font even when none has
    /// it.
    pub(crate) fn candidates(&self, query: &Query<'_>) -> Vec<(String, Source)> {
        let mut candidates = query.family.map_or_else(Vec::new, |family| of_family(self.fonts(), family, query));
        candidates.extend(self.matched(query));
        candidates
    }

    /// The system's fonts, listed the first time.
    fn fonts(&self) -> &[SystemFont] {
        self.fonts.get_or_init(|| {
            let mut fonts = Vec::new();
            // SAFETY: the iterator and each font are checked for null, used
            // while they live and released once; a font's path belongs to
            // it, and is copied before it is closed.
            unsafe {
                let iterator = (self.iterator_open)();
                if iterator.is_null() {
                    return fonts;
                }
                loop {
                    let font = (self.iterator_next)(iterator);
                    if font.is_null() {
                        break;
                    }
                    let (source, weight, italic) =
                        (self.source(font), (self.font_weight)(font), (self.font_is_italic)(font));
                    (self.close)(font);
                    if let Some(family) = source.family_name() {
                        fonts.push(SystemFont { family, weight, italic, source });
                    }
                }
                (self.iterator_close)(iterator);
            }
            fonts
        })
    }

    /// The font file and face of `font`, an `AFont`.
    ///
    /// # Safety
    ///
    /// `font` is a live `AFont`.
    unsafe fn source(&self, font: *const c_void) -> Source {
        // SAFETY: the caller's contract; the path belongs to the font, and
        // is copied.
        let path =
            unsafe { PathBuf::from(std::ffi::OsStr::from_bytes(CStr::from_ptr((self.file_path)(font)).to_bytes())) };
        // SAFETY: as above.
        let index = unsafe { (self.collection_index)(font) } as u32;
        Source::File { path, index }
    }

    /// The font that Android draws `query`'s character in, with its family
    /// name.
    fn matched(&self, query: &Query<'_>) -> Option<(String, Source)> {
        let mut probe = [0; 2];
        let probe = query.probe.encode_utf16(&mut probe);
        let locales = CString::new(query.languages.join(",")).ok()?;
        let family = CString::new(query.family.unwrap_or("sans-serif")).ok()?;
        // SAFETY: the matcher and the font are checked for null, used while
        // they live and released once; the path belongs to the font, and is
        // copied before the font is closed.
        unsafe {
            let matcher = (self.create)();
            if matcher.is_null() {
                return None;
            }
            if !query.languages.is_empty() {
                (self.set_locales)(matcher, locales.as_ptr());
            }
            (self.set_style)(matcher, query.weight, query.italic);
            let font =
                (self.matches)(matcher, family.as_ptr(), probe.as_ptr(), probe.len() as u32, std::ptr::null_mut());
            (self.destroy)(matcher);
            if font.is_null() {
                return None;
            }
            let source = self.source(font);
            (self.close)(font);
            Some((source.family_name().unwrap_or_default(), source))
        }
    }
}

/// How many of a family's fonts come before the matcher's pick, so that a
/// lookup reads the pick too.
const MAX_FAMILY_FONTS: usize = 8;

/// The fonts of `fonts` whose family is `family`, of `query`'s style if the
/// family has it, the weight closest to `query`'s first.
fn of_family(fonts: &[SystemFont], family: &str, query: &Query<'_>) -> Vec<(String, Source)> {
    let mut found: Vec<&SystemFont> = fonts.iter().filter(|font| font.family.eq_ignore_ascii_case(family)).collect();
    if found.iter().any(|font| font.italic == query.italic) {
        found.retain(|font| font.italic == query.italic);
    }
    found.sort_by_key(|font| font.weight.abs_diff(query.weight));
    found.into_iter().take(MAX_FAMILY_FONTS).map(|font| (font.family.clone(), font.source.clone())).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{FontMatcher, SystemFont, of_family};
    use crate::text::system_fonts::Query;
    use crate::text::system_fonts::source::Source;

    #[test]
    fn a_family_lists_its_fonts_of_the_style_asked_for_the_closest_weight_first() {
        let font = |family: &str, weight, italic, path: &str| SystemFont {
            family: family.to_owned(),
            weight,
            italic,
            source: Source::File { path: PathBuf::from(path), index: 0 },
        };
        let fonts = [
            font("Noto Serif", 400, false, "regular"),
            font("Noto Serif", 700, true, "bold italic"),
            font("Roboto", 700, false, "roboto"),
            font("Noto Serif", 700, false, "bold"),
        ];
        let query =
            Query { name: None, family: Some("noto serif"), probe: 'a', languages: &[], weight: 700, italic: false };
        let paths: Vec<PathBuf> = of_family(&fonts, "noto serif", &query)
            .into_iter()
            .map(|(_, source)| {
                let Source::File { path, .. } = source;
                path
            })
            .collect();
        assert_eq!(paths, ["bold", "regular"].map(PathBuf::from));
        // Of a family without the style, any.
        let query = Query { italic: true, weight: 400, ..query };
        let paths: Vec<PathBuf> = of_family(&fonts[..1], "noto serif", &query)
            .into_iter()
            .map(|(_, source)| {
                let Source::File { path, .. } = source;
                path
            })
            .collect();
        assert_eq!(paths, ["regular"].map(PathBuf::from));
    }

    /// The device's API level, from `ro.build.version.sdk`.
    fn api_level() -> u32 {
        let output = std::process::Command::new("getprop").arg("ro.build.version.sdk").output().unwrap();
        String::from_utf8_lossy(&output.stdout).trim().parse().unwrap()
    }

    #[test]
    fn the_matcher_is_there_from_api_29() {
        assert_eq!(FontMatcher::load().is_some(), api_level() >= 29);
    }

    /// A test binary is no app, whose system font collection the matcher
    /// needs, so it finds no system fonts.
    #[test]
    fn outside_an_app_the_matcher_is_left_out() {
        assert!(FontMatcher::new().is_none());
    }
}
