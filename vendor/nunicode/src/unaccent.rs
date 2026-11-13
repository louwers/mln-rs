// Ports src/libnu/tounaccent.c and include/libnu/casemap_internal.h.
//
// A transformation (C: `const char*` to a 0-terminated UTF-8 string, or
// null) is a byte slice that starts at the mapping; decoding stops at its
// 0 terminator (`nu_casemap_read` = `nu_utf8_read`).

use crate::tables::tounaccent;
use crate::udb::nu_udb_lookup;

pub type Transform = Option<&'static [u8]>;

/// Zero-terminated empty string in `nu_casemap_read` (UTF-8).
static EMPTY: &[u8] = b"\0";

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
