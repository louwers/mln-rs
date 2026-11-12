// Ports include/libnu/mph.h and include/libnu/udb.h: the minimal perfect
// hash lookups over the generated tables.

/// Those need to be the same values as used in MPH generation.
const PRIME: u32 = 0x0100_0193;

#[inline]
fn nu_hash(hash: u32, codepoint: u32) -> u32 {
    let hash = if hash == 0 { PRIME } else { hash };
    hash ^ codepoint
}

/// `nu_mph_hash`.
#[inline]
#[must_use]
pub fn nu_mph_hash(g: &[i16], codepoint: u32) -> u32 {
    let g_size = g.len() as u32;
    let h = nu_hash(0, codepoint);
    let offset = g[(h % g_size) as usize];
    if offset < 0 {
        return (-i32::from(offset) - 1) as u32;
    }
    // `_nu_hash(offset, codepoint)`: the int16_t converts to uint32_t.
    nu_hash(offset as u32, codepoint) % g_size
}

/// `nu_mph_lookup`: due to nature of minimal perfect hash, it will always
/// produce a result for codepoints that are in the table, but also for those
/// that are not; the codepoint check filters the latter.
#[inline]
#[must_use]
pub fn nu_mph_lookup(values_c: &[u32], values_i: &[u16], codepoint: u32, hash: u32) -> u32 {
    let c = values_c[hash as usize];
    let i = values_i[hash as usize];
    if c == codepoint { u32::from(i) } else { 0 }
}

/// `nu_udb_lookup_value`.
#[inline]
#[must_use]
pub fn nu_udb_lookup_value(codepoint: u32, g: &[i16], values_c: &[u32], values_i: &[u16]) -> u32 {
    let hash = nu_mph_hash(g, codepoint);
    nu_mph_lookup(values_c, values_i, codepoint, hash)
}

/// `nu_udb_lookup`: the 0-terminated UTF-8 string at the combined offset,
/// or `None` (C: a null pointer).
#[inline]
#[must_use]
pub fn nu_udb_lookup(
    codepoint: u32,
    g: &[i16],
    values_c: &[u32],
    values_i: &[u16],
    combined: &'static [u8],
) -> Option<&'static [u8]> {
    let combined_offset = nu_udb_lookup_value(codepoint, g, values_c, values_i);
    if combined_offset == 0 {
        return None;
    }
    Some(&combined[combined_offset as usize..])
}
