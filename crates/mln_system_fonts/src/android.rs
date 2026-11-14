//! The font for a script from Android's font matcher (`AFontMatcher`, API
//! level 29), which picks the system font that the platform would draw the
//! text in for the given languages. Its functions are looked up at run time,
//! so that an app that runs on older versions falls back to the scan of
//! `/system/fonts`. And the app's languages, from Java, through [`init`].
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

use crate::scripts::Script;
use crate::source::Source;

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

/// The functions of the NDK's `font_matcher.h` and `font.h` that this uses.
pub(crate) struct FontMatcher {
    create: unsafe extern "C" fn() -> *mut c_void,
    destroy: unsafe extern "C" fn(*mut c_void),
    set_locales: unsafe extern "C" fn(*mut c_void, *const c_char),
    matches: unsafe extern "C" fn(*const c_void, *const c_char, *const u16, u32, *mut u32) -> *mut c_void,
    close: unsafe extern "C" fn(*mut c_void),
    file_path: unsafe extern "C" fn(*const c_void) -> *const c_char,
    collection_index: unsafe extern "C" fn(*const c_void) -> usize,
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
            matches: symbol!("AFontMatcher_match"),
            close: symbol!("AFont_close"),
            file_path: symbol!("AFont_getFontFilePath"),
            collection_index: symbol!("AFont_getCollectionIndex"),
            _library: library,
        })
    }

    /// The font that Android draws `script` in for `languages`, with the name
    /// of its file as its family. It may lack the script's glyphs: the
    /// matcher returns a font even when none has them.
    pub(crate) fn candidates(&self, script: &Script, languages: &[String]) -> Vec<(String, Source)> {
        let mut probe = [0; 2];
        let probe = script.probe.encode_utf16(&mut probe);
        let Ok(locales) = CString::new(languages.join(",")) else { return Vec::new() };
        // SAFETY: the matcher and the font are checked for null, used while
        // they live and released once; the path belongs to the font, and is
        // copied before the font is closed.
        unsafe {
            let matcher = (self.create)();
            if matcher.is_null() {
                return Vec::new();
            }
            if !languages.is_empty() {
                (self.set_locales)(matcher, locales.as_ptr());
            }
            let font = (self.matches)(
                matcher,
                c"sans-serif".as_ptr(),
                probe.as_ptr(),
                probe.len() as u32,
                std::ptr::null_mut(),
            );
            (self.destroy)(matcher);
            if font.is_null() {
                return Vec::new();
            }
            let path = PathBuf::from(std::ffi::OsStr::from_bytes(CStr::from_ptr((self.file_path)(font)).to_bytes()));
            let index = (self.collection_index)(font) as u32;
            (self.close)(font);
            let family = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
            vec![(family, Source::File { path, index })]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FontMatcher;

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
    /// needs, so the fonts come from the scan.
    #[test]
    fn outside_an_app_the_matcher_is_left_out() {
        assert!(FontMatcher::new().is_none());
    }
}
