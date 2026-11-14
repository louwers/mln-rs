//! The font for a script from DirectWrite's system font fallback
//! (`IDWriteFontFallback::MapCharacters`): the font Windows draws a letter of
//! the script in, for the user's language, when the font asked for lacks it.

#![expect(unsafe_code, reason = "DirectWrite is the FFI here")]

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWriteCreateFactory, IDWriteFactory2, IDWriteFont, IDWriteFontFallback, IDWriteLocalFontFileLoader,
    IDWriteTextAnalysisSource,
};
use windows::core::{Interface, PCWSTR};

use self::probe::Probe;
use crate::scripts::Script;
use crate::source::Source;

/// DirectWrite's system font fallback.
pub(crate) struct DirectWrite {
    fallback: IDWriteFontFallback,
}

impl DirectWrite {
    /// The fallback, or `None` before Windows 8.1.
    pub(crate) fn new() -> Option<DirectWrite> {
        // SAFETY: no arguments but the factory type.
        let factory: IDWriteFactory2 = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.ok()?;
        // SAFETY: a valid factory.
        let fallback = unsafe { factory.GetSystemFontFallback() }.ok()?;
        Some(DirectWrite { fallback })
    }

    /// The font that Windows draws `script` in for the first of `languages`,
    /// or for the user's language when there are none.
    pub(crate) fn candidates(&self, script: &Script, languages: &[String]) -> Vec<(String, Source)> {
        self.font_for(script, languages).into_iter().collect()
    }

    fn font_for(&self, script: &Script, languages: &[String]) -> Option<(String, Source)> {
        let locale = languages.first().cloned().or_else(sys_locale::get_locale).unwrap_or_default();
        let mut probe = [0; 2];
        let text = script.probe.encode_utf16(&mut probe).to_vec();
        let length = text.len() as u32;
        let source: IDWriteTextAnalysisSource =
            Probe { text, locale: locale.encode_utf16().chain([0]).collect() }.into();
        let (mut mapped, mut font, mut scale) = (0, None, 0.0);
        // SAFETY: valid objects, and out-pointers to locals; a null base family
        // matches no family.
        unsafe {
            self.fallback
                .MapCharacters(
                    &source,
                    0,
                    length,
                    None,
                    PCWSTR::null(),
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    &raw mut mapped,
                    &raw mut font,
                    &raw mut scale,
                )
                .ok()?;
        }
        let font = font?;
        Some((family_name(&font).unwrap_or_default(), file(&font)?))
    }
}

/// The font's family name, in the first language it has it in.
fn family_name(font: &IDWriteFont) -> Option<String> {
    // SAFETY: valid objects, and a buffer of the length asked for.
    unsafe {
        let names = font.GetFontFamily().ok()?.GetFamilyNames().ok()?;
        let length = names.GetStringLength(0).ok()? as usize;
        let mut name = vec![0; length + 1];
        names.GetString(0, &mut name).ok()?;
        Some(String::from_utf16_lossy(&name[..length]))
    }
}

/// The font's file and its face in it, when it is a local file.
fn file(font: &IDWriteFont) -> Option<Source> {
    // SAFETY: valid objects; the face has one file; the key belongs to the
    // file, which outlives its use; the path buffer has the length asked for.
    unsafe {
        let face = font.CreateFontFace().ok()?;
        let mut count = 1;
        let mut files = [None];
        face.GetFiles(&raw mut count, Some(files.as_mut_ptr())).ok()?;
        let file = files[0].take()?;
        let (mut key, mut key_size) = (ptr::null_mut(), 0);
        file.GetReferenceKey(&raw mut key, &raw mut key_size).ok()?;
        let loader: IDWriteLocalFontFileLoader = file.GetLoader().ok()?.cast().ok()?;
        let length = loader.GetFilePathLengthFromKey(key, key_size).ok()? as usize;
        let mut path = vec![0; length + 1];
        loader.GetFilePathFromKey(key, key_size, &mut path).ok()?;
        let path = PathBuf::from(OsString::from_wide(&path[..length]));
        Some(Source::File { path, index: face.GetIndex() })
    }
}

/// The text that DirectWrite's fallback maps, in a module of its own for the
/// lints of the COM glue that `#[implement]` generates.
#[expect(trivial_casts, clippy::ref_as_ptr, clippy::inline_always, reason = "the code that #[implement] generates")]
mod probe {
    use std::ptr;

    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_READING_DIRECTION, DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, IDWriteNumberSubstitution,
        IDWriteTextAnalysisSource, IDWriteTextAnalysisSource_Impl,
    };
    use windows::core::{OutRef, Result, implement};

    /// One letter in one language, as text that DirectWrite analyzes.
    #[implement(IDWriteTextAnalysisSource)]
    pub(super) struct Probe {
        pub(super) text: Vec<u16>,
        /// The language's BCP 47 tag, NUL-terminated.
        pub(super) locale: Vec<u16>,
    }

    impl IDWriteTextAnalysisSource_Impl for Probe_Impl {
        fn GetTextAtPosition(&self, position: u32, text: *mut *mut u16, length: *mut u32) -> Result<()> {
            let rest = self.text.get(position as usize..).unwrap_or_default();
            // SAFETY: DirectWrite passes valid out-pointers, and reads the text
            // only while the source lives.
            unsafe {
                *text = if rest.is_empty() { ptr::null_mut() } else { rest.as_ptr().cast_mut() };
                *length = rest.len() as u32;
            }
            Ok(())
        }

        fn GetTextBeforePosition(&self, position: u32, text: *mut *mut u16, length: *mut u32) -> Result<()> {
            let before = self.text.get(..position as usize).unwrap_or_default();
            // SAFETY: as in `GetTextAtPosition`.
            unsafe {
                *text = if before.is_empty() { ptr::null_mut() } else { before.as_ptr().cast_mut() };
                *length = before.len() as u32;
            }
            Ok(())
        }

        fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
            DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
        }

        fn GetLocaleName(&self, position: u32, length: *mut u32, locale: *mut *mut u16) -> Result<()> {
            // SAFETY: as in `GetTextAtPosition`.
            unsafe {
                *length = (self.text.len() as u32).saturating_sub(position);
                *locale = self.locale.as_ptr().cast_mut();
            }
            Ok(())
        }

        fn GetNumberSubstitution(
            &self,
            position: u32,
            length: *mut u32,
            substitution: OutRef<IDWriteNumberSubstitution>,
        ) -> Result<()> {
            // SAFETY: as in `GetTextAtPosition`.
            unsafe { *length = (self.text.len() as u32).saturating_sub(position) };
            substitution.write(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DirectWrite;
    use crate::scripts::SCRIPTS;

    #[test]
    fn every_script_has_a_font_that_draws_it() {
        let fallback = DirectWrite::new().expect("DirectWrite's font fallback");
        for script in SCRIPTS {
            let languages = script.language.map(|language| vec![language.to_owned()]).unwrap_or_default();
            let found = fallback.candidates(script, &languages);
            println!("{:04X}: {:?}", u32::from(script.probe), found.first());
            let (_, source) = found.first().unwrap_or_else(|| panic!("no font for {:04X}", u32::from(script.probe)));
            assert!(source.drawn_weight(script.probe).is_some());
        }
    }

    #[test]
    fn ideographs_take_a_font_for_the_users_language() {
        let fallback = DirectWrite::new().unwrap();
        let han = SCRIPTS.iter().find(|script| script.probe == '\u{4E2D}').unwrap();
        let first = |language: &str| fallback.candidates(han, &[language.to_owned()]).remove(0);
        let (japanese, chinese) = (first("ja-JP"), first("zh-CN"));
        println!("ja-JP: {japanese:?}\nzh-CN: {chinese:?}");
        assert_ne!(japanese.1, chinese.1);
    }
}
