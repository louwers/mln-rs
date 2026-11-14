//! The fonts for a script from fontconfig, the font matcher of Linux and the
//! BSDs, loaded at run time so that a system without it falls back to the
//! scan of font files.
//!
//! For each script, fontconfig sorts the system's fonts against
//! `sans-serif`, the script's language and a letter of it, applying the
//! user's and the system's font configuration. The letter outranks the
//! family and the language, so the fonts that have it come first, and of
//! those, the families that the configuration prefers for the language.

#![expect(unsafe_code, reason = "fontconfig's C API is the FFI here")]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr;

use libloading::Library;

use crate::scripts::Script;
use crate::source::Source;

/// How many of the sorted fonts that have the script's letter to try.
const MAX_CANDIDATES: usize = 8;

type FcBool = c_int;
type FcResult = c_int;
const FC_RESULT_MATCH: FcResult = 0;
const FC_MATCH_PATTERN: c_int = 0;

#[repr(C)]
struct FcFontSet {
    nfont: c_int,
    sfont: c_int,
    fonts: *mut *mut c_void,
}

/// The functions of fontconfig's C API that this uses.
struct Api {
    init_load_config_and_fonts: unsafe extern "C" fn() -> *mut c_void,
    config_destroy: unsafe extern "C" fn(*mut c_void),
    name_parse: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    pattern_destroy: unsafe extern "C" fn(*mut c_void),
    pattern_add_char_set: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_void) -> FcBool,
    pattern_get_string: unsafe extern "C" fn(*const c_void, *const c_char, c_int, *mut *const c_char) -> FcResult,
    pattern_get_integer: unsafe extern "C" fn(*const c_void, *const c_char, c_int, *mut c_int) -> FcResult,
    pattern_get_char_set: unsafe extern "C" fn(*const c_void, *const c_char, c_int, *mut *mut c_void) -> FcResult,
    char_set_create: unsafe extern "C" fn() -> *mut c_void,
    char_set_destroy: unsafe extern "C" fn(*mut c_void),
    char_set_add_char: unsafe extern "C" fn(*mut c_void, u32) -> FcBool,
    char_set_has_char: unsafe extern "C" fn(*const c_void, u32) -> FcBool,
    config_substitute: unsafe extern "C" fn(*mut c_void, *mut c_void, c_int) -> FcBool,
    default_substitute: unsafe extern "C" fn(*mut c_void),
    font_sort:
        unsafe extern "C" fn(*mut c_void, *mut c_void, FcBool, *mut *mut c_void, *mut FcResult) -> *mut FcFontSet,
    font_set_destroy: unsafe extern "C" fn(*mut FcFontSet),
}

/// fontconfig, with its configuration and the system's fonts loaded.
pub(crate) struct Fontconfig {
    api: Api,
    config: *mut c_void,
    // Last, so that the functions above stay loaded while they are used.
    _library: Library,
}

impl Fontconfig {
    /// Loads fontconfig, or `None` where the system lacks it.
    pub(crate) fn new() -> Option<Fontconfig> {
        // SAFETY: loading fontconfig runs no initializers that need anything
        // of the caller.
        let library = ["libfontconfig.so.1", "libfontconfig.so"]
            .into_iter()
            .find_map(|name| unsafe { Library::new(name) }.ok())?;
        macro_rules! symbol {
            ($name:literal) => {
                // SAFETY: the signature is that of the function in
                // fontconfig.h.
                *unsafe { library.get(concat!($name, "\0").as_bytes()) }.ok()?
            };
        }
        let api = Api {
            init_load_config_and_fonts: symbol!("FcInitLoadConfigAndFonts"),
            config_destroy: symbol!("FcConfigDestroy"),
            name_parse: symbol!("FcNameParse"),
            pattern_destroy: symbol!("FcPatternDestroy"),
            pattern_add_char_set: symbol!("FcPatternAddCharSet"),
            pattern_get_string: symbol!("FcPatternGetString"),
            pattern_get_integer: symbol!("FcPatternGetInteger"),
            pattern_get_char_set: symbol!("FcPatternGetCharSet"),
            char_set_create: symbol!("FcCharSetCreate"),
            char_set_destroy: symbol!("FcCharSetDestroy"),
            char_set_add_char: symbol!("FcCharSetAddChar"),
            char_set_has_char: symbol!("FcCharSetHasChar"),
            config_substitute: symbol!("FcConfigSubstitute"),
            default_substitute: symbol!("FcDefaultSubstitute"),
            font_sort: symbol!("FcFontSort"),
            font_set_destroy: symbol!("FcFontSetDestroy"),
        };
        // SAFETY: no arguments; the configuration is this value's to destroy.
        let config = unsafe { (api.init_load_config_and_fonts)() };
        if config.is_null() {
            return None;
        }
        Some(Fontconfig { api, config, _library: library })
    }

    /// The fonts that may draw `script` for `languages`, best first.
    pub(crate) fn candidates(&self, script: &Script, languages: &[String]) -> Vec<(String, Source)> {
        let api = &self.api;
        let mut name = String::from("sans-serif");
        if let Some(language) = languages.first().map(|tag| fontconfig_language(tag)).filter(|tag| !tag.is_empty()) {
            name.push_str(":lang=");
            name.push_str(&language);
        }
        let Ok(name) = CString::new(name) else { return Vec::new() };
        let probe = u32::from(script.probe);
        let mut candidates = Vec::new();
        // SAFETY: each object is checked for null, used while it lives and
        // destroyed once; the font set's patterns belong to the set, and the
        // strings and character sets read from them to the patterns.
        unsafe {
            let pattern = (api.name_parse)(name.as_ptr());
            if pattern.is_null() {
                return candidates;
            }
            let charset = (api.char_set_create)();
            if !charset.is_null() {
                (api.char_set_add_char)(charset, probe);
                (api.pattern_add_char_set)(pattern, c"charset".as_ptr(), charset);
                (api.char_set_destroy)(charset);
            }
            (api.config_substitute)(self.config, pattern, FC_MATCH_PATTERN);
            (api.default_substitute)(pattern);
            let mut result = 0;
            let set = (api.font_sort)(self.config, pattern, 0, ptr::null_mut(), &raw mut result);
            (api.pattern_destroy)(pattern);
            if set.is_null() {
                return candidates;
            }
            let fonts = (*set).fonts;
            for i in 0..usize::try_from((*set).nfont).unwrap_or(0) {
                let font = *fonts.add(i);
                let mut charset = ptr::null_mut();
                let has_probe = (api.pattern_get_char_set)(font, c"charset".as_ptr(), 0, &raw mut charset)
                    == FC_RESULT_MATCH
                    && (api.char_set_has_char)(charset, probe) != 0;
                if !has_probe {
                    continue;
                }
                let mut file = ptr::null();
                if (api.pattern_get_string)(font, c"file".as_ptr(), 0, &raw mut file) != FC_RESULT_MATCH {
                    continue;
                }
                let path = PathBuf::from(std::ffi::OsStr::from_bytes(CStr::from_ptr(file).to_bytes()));
                let mut index = 0;
                (api.pattern_get_integer)(font, c"index".as_ptr(), 0, &raw mut index);
                // The high 16 bits number a variable font's named instance;
                // the map picks the font's weight itself.
                let source = Source::File { path, index: (index as u32) & 0xFFFF };
                let mut family = ptr::null();
                let family =
                    if (api.pattern_get_string)(font, c"family".as_ptr(), 0, &raw mut family) == FC_RESULT_MATCH {
                        CStr::from_ptr(family).to_string_lossy().into_owned()
                    } else {
                        String::new()
                    };
                // Not a monospaced family, as the scan does: fontconfig
                // leaves the spacing of some (Noto Sans Mono CJK) unset.
                if family.contains("Mono") || candidates.iter().any(|(_, known)| *known == source) {
                    continue;
                }
                candidates.push((family, source));
                if candidates.len() == MAX_CANDIDATES {
                    break;
                }
            }
            (api.font_set_destroy)(set);
        }
        candidates
    }
}

impl Drop for Fontconfig {
    fn drop(&mut self) {
        // SAFETY: the configuration is this value's, and nothing uses it after.
        unsafe { (self.api.config_destroy)(self.config) };
    }
}

/// A BCP 47 tag as fontconfig names its language: lowercase, with a region
/// rather than a script for Chinese (`zh-Hant` is `zh-tw`), and nothing
/// fontconfig's names can't hold.
fn fontconfig_language(tag: &str) -> String {
    let tag = tag.replace('_', "-").to_ascii_lowercase();
    let mut subtags = tag.split('-');
    let primary = subtags.next().unwrap_or_default();
    let rest: Vec<&str> = subtags.collect();
    let language = if primary == "zh" {
        let region = rest.iter().find(|subtag| subtag.len() == 2 && subtag.chars().all(|c| c.is_ascii_alphabetic()));
        match (rest.contains(&"hant"), region) {
            (_, Some(region)) => format!("zh-{region}"),
            (true, None) => "zh-tw".to_owned(),
            (false, None) => "zh-cn".to_owned(),
        }
    } else {
        match rest.iter().find(|subtag| subtag.len() == 2) {
            Some(region) => format!("{primary}-{region}"),
            None => primary.to_owned(),
        }
    };
    language.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect()
}

#[cfg(test)]
mod tests {
    use super::fontconfig_language;

    #[test]
    fn languages_take_fontconfigs_names() {
        assert_eq!(fontconfig_language("ja-JP"), "ja-jp");
        assert_eq!(fontconfig_language("ja"), "ja");
        assert_eq!(fontconfig_language("zh-Hans"), "zh-cn");
        assert_eq!(fontconfig_language("zh-Hant"), "zh-tw");
        assert_eq!(fontconfig_language("zh-Hant-HK"), "zh-hk");
        assert_eq!(fontconfig_language("zh_TW"), "zh-tw");
        assert_eq!(fontconfig_language("sr-Latn-RS"), "sr-rs");
        assert_eq!(fontconfig_language("en:x"), "enx");
    }
}
