//! Ports the vendored nunicode subset of the C++ core
//! (`vendor/nunicode`, with its `config.h`: UTF-8, unaccent and
//! 0-terminated collation), plus the `unaccent` helper of the default
//! platform's collator (platform/default/src/mln/i18n/collator.cpp).
//!
//! Case-insensitive collation folds case with `char::to_uppercase`.
//!
//! The generated tables (`src/tables`) come from nunicode's `src/libnu/gen`
//! through `tools/gen_tables.py`.

pub mod ducet;
pub mod strcoll;
mod tables;
pub mod udb;
pub mod unaccent;
pub mod utf8;

pub use strcoll::{nu_strcasecoll, nu_strcoll};
use unaccent::nu_tounaccent;
use utf8::{nu_utf8_read, nu_utf8_write};

/// collator.cpp's `unaccent` loop: over the whole string (not up to a NUL),
/// copying codepoints without a mapping and writing the decoded mapping
/// otherwise.
#[must_use]
pub fn unaccent_bytes(s: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(s.len());
    let end = s.len();
    let mut itr = 0;

    while itr < end {
        let (nitr, code_point) = nu_utf8_read(s, itr);
        if let Some(mut buf) = nu_tounaccent(code_point) {
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

/// `unaccent_bytes`'s result as a `String`. nunicode's `b4_utf8`
/// (`utf8_internal.h`) puts bit 12 of a 4-byte codepoint into the third byte
/// instead of the second, so a mapping to such a codepoint comes out as
/// invalid UTF-8. A `String` cannot hold that; those bytes become U+FFFD.
fn into_string(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
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
/// case-insensitive through uppercase folding, diacritic-insensitive through
/// `unaccent`. Works on the exact bytes, so it matches C++ also where
/// `unaccent` produces invalid UTF-8.
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
    fn unaccent_() {
        assert_eq!(unaccent("äéñ"), "aen");
        assert_eq!(unaccent("a\0b"), "a\0b");
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
        assert_eq!(strcasecoll("straße", "STRASSE"), 0);
        assert!(strcasecoll("straße", "strasse!") < 0);
        assert!(strcoll("", "a") < 0);
        assert_eq!(strcoll("a\0b", "a\0c"), 0);
    }
}
