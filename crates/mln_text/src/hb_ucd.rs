// Ports the general category and script lookups of
// ../maplibre-native/vendor/harfbuzz/src/hb-ucd.cc (`hb_ucd_general_category`,
// `hb_ucd_script`) with the tables HB_TINY compiles (Unicode 15.0.0,
// generated into hb_ucd_table.rs by tools/gen_hb_ucd.py). The C++ core's
// `getUnicodeScript` (harfbuzz_impl.cpp) uses them through
// `hb_unicode_funcs_get_default()`; HarfRust's own tables are Unicode 18.
// Under HarfBuzz's "Old MIT" license (LICENSE-HarfBuzz); the tables' data is
// Unicode's, under the Unicode License V3 (the first part of LICENSE-ICU).

use crate::hb_ucd_table;

/// `HB_UNICODE_GENERAL_CATEGORY_NON_SPACING_MARK` (`hb_unicode_general_category_t`).
pub const NON_SPACING_MARK: u8 = 12;

/// `hb_unicode_general_category` with the default (UCD) funcs.
#[must_use]
pub fn general_category(u: u32) -> u8 {
    hb_ucd_table::gc(u)
}

/// `hb_unicode_script` with the default (UCD) funcs, as an ISO 15924 tag.
#[must_use]
pub fn script(u: u32) -> [u8; 4] {
    hb_ucd_table::SC_MAP[hb_ucd_table::sc(u) as usize]
}
