// Ports src/libnu/toupper.c, src/libnu/tolower.c, src/libnu/tounaccent.c
// and include/libnu/casemap_internal.h.
//
// A transformation (C: `const char*` to a 0-terminated UTF-8 string, or
// null) is a byte slice that starts at the mapping; decoding stops at its
// 0 terminator (`nu_casemap_read` = `nu_utf8_read`).

use crate::tables::{tolower, tounaccent, toupper};
use crate::udb::nu_udb_lookup;
use crate::utf8::nu_utf8_read;

pub type Transform = Option<&'static [u8]>;

/// In `nu_casemap_read` (UTF-8), zero-terminated.
static NU_FINAL_SIGMA: &[u8] = "ς\0".as_bytes();

/// Zero-terminated empty string in `nu_casemap_read` (UTF-8).
static EMPTY: &[u8] = b"\0";

/// `nu_toupper`.
#[must_use]
pub fn nu_toupper(codepoint: u32) -> Transform {
    nu_udb_lookup(
        codepoint,
        &toupper::NU_TOUPPER_G,
        &toupper::NU_TOUPPER_VALUES_C,
        &toupper::NU_TOUPPER_VALUES_I,
        &toupper::NU_TOUPPER_COMBINED,
    )
}

/// `_nu_toupper`: reads the codepoint at `i` of `s` (limit `limit`), returns
/// the next position, the codepoint and its transformation.
#[must_use]
pub fn _nu_toupper(s: &[u8], i: usize, _limit: usize) -> (usize, u32, Transform) {
    let (np, u) = nu_utf8_read(s, i);
    (np, u, nu_toupper(u))
}

/// `nu_tolower`.
#[must_use]
pub fn nu_tolower(codepoint: u32) -> Transform {
    nu_udb_lookup(
        codepoint,
        &tolower::NU_TOLOWER_G,
        &tolower::NU_TOLOWER_VALUES_C,
        &tolower::NU_TOLOWER_VALUES_I,
        &tolower::NU_TOLOWER_COMBINED,
    )
}

/// `_nu_tolower`.
#[must_use]
pub fn _nu_tolower(s: &[u8], i: usize, limit: usize) -> (usize, u32, Transform) {
    let (np, u) = nu_utf8_read(s, i);

    // handling of 0x03A3 ('Σ')
    //
    // this is the only language-independent exception described in
    // SpecialCasing.txt (Unicode 7.0)
    if u == 0x03A3 {
        if np >= limit {
            return (np, u, Some(NU_FINAL_SIGMA));
        }
        let (_, nu) = nu_utf8_read(s, np);
        if nu == 0 {
            return (np, u, Some(NU_FINAL_SIGMA));
        }
    }

    (np, u, nu_tolower(u))
}

/// `nu_tounaccent`.
#[must_use]
pub fn nu_tounaccent(codepoint: u32) -> Transform {
    const BLOCKS: [(u32, u32); 4] = [
        (0x0300, 0x036F), // Combining Diacritical Marks
        (0x1AB0, 0x1AFF), // Combining Diacritical Marks Extended
        (0x20D0, 0x20FF), // Combining Diacritical Marks for Symbols
        (0x1DC0, 0x1DFF), // Combining Diacritical Marks Supplement
    ];

    // check if codepoint itself is a diacritic, return empty string in that
    // case (transform into empty string)
    for (block_start, block_end) in BLOCKS {
        if codepoint >= block_start && codepoint <= block_end {
            return Some(EMPTY);
        }
    }

    nu_udb_lookup(
        codepoint,
        &tounaccent::NU_TOUNACCENT_G,
        &tounaccent::NU_TOUNACCENT_VALUES_C,
        &tounaccent::NU_TOUNACCENT_VALUES_I,
        &tounaccent::NU_TOUNACCENT_COMBINED,
    )
}

/// `_nu_tounaccent`.
#[must_use]
pub fn _nu_tounaccent(s: &[u8], i: usize, _limit: usize) -> (usize, u32, Transform) {
    let (np, u) = nu_utf8_read(s, i);
    (np, u, nu_tounaccent(u))
}
