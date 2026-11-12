//! Ports the vendored nunicode subset of the C++ core
//! (`vendor/nunicode`, with its `config.h`: UTF-8, toupper, tolower,
//! unaccent and 0-terminated collation), plus the three functions the
//! default platform builds on it:
//!
//! - `unaccent` from platform/default/src/mln/i18n/collator.cpp,
//! - `uppercase`/`lowercase` from `platform/default/src/mln/util/string_stdlib.cpp`.
//!
//! The generated tables (`src/tables`) come from nunicode's `src/libnu/gen`
//! through `tools/gen_tables.py`.

pub mod casemap;
pub mod ducet;
pub mod strcoll;
mod tables;
pub mod udb;
pub mod utf8;

use casemap::{_nu_tolower, _nu_tounaccent, _nu_toupper, Transform};
pub use strcoll::{nu_strcasecoll, nu_strcoll};
use utf8::{nu_utf8_read, nu_utf8_write};

type TransformFn = fn(&[u8], usize, usize) -> (usize, u32, Transform);

/// The loop shared by `string_stdlib.cpp`'s `uppercase`/`lowercase` and
/// collator.cpp's `unaccent`: over the whole string (not up to a NUL),
/// copying codepoints without a mapping and writing the decoded mapping
/// otherwise.
fn transform_bytes(s: &[u8], f: TransformFn) -> Vec<u8> {
    let mut output = Vec::with_capacity(s.len());
    let end = s.len();
    let mut itr = 0;

    while itr < end {
        let (nitr, _code_point, buf) = f(s, itr, end);
        if let Some(mut buf) = buf {
            loop {
                let (next, code_point) = nu_utf8_read(buf, 0);
                buf = &buf[next.min(buf.len())..];
                if code_point == 0 {
                    break;
                }
                nu_utf8_write(code_point, &mut output);
            }
        } else {
            output.extend_from_slice(&s[itr..nitr.min(end)]);
        }
        itr = nitr;
    }

    output
}

/// The `_bytes` results as a `String`. nunicode's `b4_utf8` (`utf8_internal.h`)
/// puts bit 12 of a 4-byte codepoint into the third byte instead of the
/// second, so mappings to such codepoints (e.g. Warang Citi U+118A0 ->
/// U+118C0) come out as invalid UTF-8 in C++. A `String` cannot hold that;
/// those bytes become U+FFFD here. The `_bytes`
/// functions return the exact C++ bytes.
fn into_string(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// `platform::uppercase` (`string_stdlib.cpp`) on raw bytes.
pub fn uppercase_bytes(s: &[u8]) -> Vec<u8> {
    transform_bytes(s, _nu_toupper)
}

/// `platform::lowercase` (`string_stdlib.cpp`) on raw bytes.
pub fn lowercase_bytes(s: &[u8]) -> Vec<u8> {
    transform_bytes(s, _nu_tolower)
}

/// `unaccent` (collator.cpp) on raw bytes.
pub fn unaccent_bytes(s: &[u8]) -> Vec<u8> {
    transform_bytes(s, _nu_tounaccent)
}

/// `platform::uppercase` (`platform/default/src/mln/util/string_stdlib.cpp`).
#[must_use]
pub fn uppercase(s: &str) -> String {
    into_string(uppercase_bytes(s.as_bytes()))
}

/// `platform::lowercase` (`platform/default/src/mln/util/string_stdlib.cpp`).
#[must_use]
pub fn lowercase(s: &str) -> String {
    into_string(lowercase_bytes(s.as_bytes()))
}

/// The `unaccent` helper of platform/default/src/mln/i18n/collator.cpp.
#[must_use]
pub fn unaccent(s: &str) -> String {
    into_string(unaccent_bytes(s.as_bytes()))
}

/// `nu_strcoll(lhs.c_str(), rhs.c_str(), nu_utf8_read, nu_utf8_read)`: an
/// embedded NUL ends the string.
#[must_use]
pub fn strcoll(lhs: &str, rhs: &str) -> i32 {
    nu_strcoll(lhs.as_bytes(), rhs.as_bytes())
}

/// `nu_strcasecoll(lhs.c_str(), rhs.c_str(), nu_utf8_read, nu_utf8_read)`.
#[must_use]
pub fn strcasecoll(lhs: &str, rhs: &str) -> i32 {
    nu_strcasecoll(lhs.as_bytes(), rhs.as_bytes())
}

/// `platform::Collator::Impl::compare` (collator.cpp): DUCET collation,
/// case-insensitive through nunicode's case folding (toupper in this
/// configuration), diacritic-insensitive through `unaccent`. Works on the
/// exact bytes, so it matches C++ also where `unaccent` produces invalid
/// UTF-8.
#[must_use]
pub fn collator_compare(lhs: &str, rhs: &str, case_sensitive: bool, diacritic_sensitive: bool) -> i32 {
    let (l, r) = (lhs.as_bytes(), rhs.as_bytes());
    if case_sensitive && diacritic_sensitive {
        nu_strcoll(l, r)
    } else if !case_sensitive && diacritic_sensitive {
        nu_strcasecoll(l, r)
    } else if case_sensitive && !diacritic_sensitive {
        nu_strcoll(&unaccent_bytes(l), &unaccent_bytes(r))
    } else {
        nu_strcasecoll(&unaccent_bytes(l), &unaccent_bytes(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_mapping() {
        assert_eq!(uppercase("abc ß ǅ"), "ABC SS Ǆ");
        assert_eq!(lowercase("ABC ΣΑΣ"), "abc σας");
        assert_eq!(lowercase("Σ"), "ς");
        assert_eq!(unaccent("äéñ"), "aen");
        assert_eq!(uppercase("a\0b"), "A\0B");
    }

    #[test]
    fn collation() {
        assert!(strcoll("a", "b") < 0);
        assert!(strcoll("b", "a") > 0);
        assert_eq!(strcoll("abc", "abc"), 0);
        assert!(strcoll("a", "A") < 0);
        assert!(strcoll("a", "ä") < 0);
        assert!(strcoll("ä", "b") < 0);
        assert_eq!(strcasecoll("a", "A"), 0);
        assert!(strcoll("", "a") < 0);
        assert_eq!(strcoll("a\0b", "a\0c"), 0);
    }
}
