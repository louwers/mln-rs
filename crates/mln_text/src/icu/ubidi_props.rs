// ICU 78.3's BiDi property getters (ubidi_props.cpp), with the `UTRIE2_GET16`
// lookup of utrie2.h. The data is ICU's compiled-in `ubidi_props_singleton`
// (ubidi_props_data.rs). The BiDi code uses the mirroring; the other getters
// serve the difftest's comparison of classes with unicode-bidi.

use super::ubidi_props_data::{
    TRIE_DATA_LENGTH, TRIE_HIGH_START, TRIE_HIGH_VALUE_INDEX, TRIE_INDEX_LENGTH, UBIDI_PROPS_INDEXES,
    UBIDI_PROPS_MIRRORS, UBIDI_PROPS_TRIE_INDEX,
};

// utrie2.h
const UTRIE2_SHIFT_1: u32 = 6 + 5;
const UTRIE2_SHIFT_2: u32 = 5;
const UTRIE2_SHIFT_1_2: u32 = UTRIE2_SHIFT_1 - UTRIE2_SHIFT_2;
const UTRIE2_OMITTED_BMP_INDEX_1_LENGTH: usize = 0x10000 >> UTRIE2_SHIFT_1;
const UTRIE2_INDEX_2_BLOCK_LENGTH: u32 = 1 << UTRIE2_SHIFT_1_2;
const UTRIE2_INDEX_2_MASK: u32 = UTRIE2_INDEX_2_BLOCK_LENGTH - 1;
const UTRIE2_DATA_BLOCK_LENGTH: u32 = 1 << UTRIE2_SHIFT_2;
const UTRIE2_DATA_MASK: u32 = UTRIE2_DATA_BLOCK_LENGTH - 1;
const UTRIE2_INDEX_SHIFT: u32 = 2;
const UTRIE2_LSCP_INDEX_2_OFFSET: usize = 0x10000 >> UTRIE2_SHIFT_2;
const UTRIE2_LSCP_INDEX_2_LENGTH: usize = 0x400 >> UTRIE2_SHIFT_2;
const UTRIE2_INDEX_2_BMP_LENGTH: usize = UTRIE2_LSCP_INDEX_2_OFFSET + UTRIE2_LSCP_INDEX_2_LENGTH;
const UTRIE2_UTF8_2B_INDEX_2_OFFSET: usize = UTRIE2_INDEX_2_BMP_LENGTH;
const UTRIE2_UTF8_2B_INDEX_2_LENGTH: usize = 0x800 >> 6;
const UTRIE2_INDEX_1_OFFSET: usize = UTRIE2_UTF8_2B_INDEX_2_OFFSET + UTRIE2_UTF8_2B_INDEX_2_LENGTH;
const UTRIE2_BAD_UTF8_DATA_OFFSET: usize = 0x80;

#[inline]
fn index_raw(index: &[u16], offset: usize, c: u32) -> usize {
    ((index[offset + (c >> UTRIE2_SHIFT_2) as usize] as usize) << UTRIE2_INDEX_SHIFT) + (c & UTRIE2_DATA_MASK) as usize
}

/// `UTRIE2_GET16(&ubidi_props_singleton.trie, c)` (`_UTRIE2_INDEX_FROM_CP`
/// with asciiOffset = indexLength).
fn trie_get16(c: i32) -> u16 {
    let index = &UBIDI_PROPS_TRIE_INDEX[..];
    debug_assert_eq!(index.len(), TRIE_INDEX_LENGTH + TRIE_DATA_LENGTH);
    let c = c as u32;
    let i = if c < 0xd800 {
        index_raw(index, 0, c)
    } else if c <= 0xffff {
        let offset = if c <= 0xdbff { UTRIE2_LSCP_INDEX_2_OFFSET - (0xd800 >> UTRIE2_SHIFT_2) } else { 0 };
        index_raw(index, offset, c)
    } else if c > 0x0010_ffff {
        TRIE_INDEX_LENGTH + UTRIE2_BAD_UTF8_DATA_OFFSET
    } else if c >= TRIE_HIGH_START {
        TRIE_HIGH_VALUE_INDEX
    } else {
        // _UTRIE2_INDEX_FROM_SUPP
        let i1 = index[(UTRIE2_INDEX_1_OFFSET - UTRIE2_OMITTED_BMP_INDEX_1_LENGTH) + (c >> UTRIE2_SHIFT_1) as usize]
            as usize;
        ((index[i1 + ((c >> UTRIE2_SHIFT_2) & UTRIE2_INDEX_2_MASK) as usize] as usize) << UTRIE2_INDEX_SHIFT)
            + (c & UTRIE2_DATA_MASK) as usize
    };
    index[i]
}

// ubidi_props.h: indexes into indexes[]
const UBIDI_IX_MIRROR_LENGTH: usize = 3;

// definitions for 16-bit bidi/shaping properties word
#[cfg(test)]
const UBIDI_BPT_SHIFT: u32 = 8;
const UBIDI_MIRROR_DELTA_SHIFT: u32 = 13;
const UBIDI_CLASS_MASK: u16 = 0x0000_001f;
#[cfg(test)]
const UBIDI_BPT_MASK: u16 = 0x0000_0300;
const UBIDI_ESC_MIRROR_DELTA: i32 = -4;
const UBIDI_MIRROR_INDEX_SHIFT: u32 = 21;

/// `UBIDI_GET_MIRROR_DELTA(props)` (arithmetic shift of the signed word).
#[inline]
fn get_mirror_delta(props: u16) -> i32 {
    i32::from((props as i16) >> UBIDI_MIRROR_DELTA_SHIFT)
}

/// `UBIDI_GET_MIRROR_CODE_POINT(m)`.
#[inline]
fn get_mirror_code_point(m: u32) -> i32 {
    (m & 0x001f_ffff) as i32
}

#[cfg(test)]
pub(crate) const U_BPT_NONE: u32 = 0;
#[cfg(test)]
pub(crate) const U_BPT_OPEN: u32 = 1;
#[cfg(test)]
pub(crate) const U_BPT_CLOSE: u32 = 2;

/// The BiDi class of `c`, a `UCharDirection` (ICU's `u_charDirection`).
#[must_use]
pub(crate) fn ubidi_get_class(c: i32) -> u8 {
    let props = trie_get16(c);
    (props & UBIDI_CLASS_MASK) as u8
}

fn get_mirror(c: i32, props: u16) -> i32 {
    let delta = get_mirror_delta(props);
    if delta == UBIDI_ESC_MIRROR_DELTA {
        // look for mirror code point in the mirrors[] table
        let mirrors = &UBIDI_PROPS_MIRRORS;
        let length = UBIDI_PROPS_INDEXES[UBIDI_IX_MIRROR_LENGTH];
        // linear search
        for i in 0..length as usize {
            let m = mirrors[i];
            let c2 = get_mirror_code_point(m);
            if c == c2 {
                // found c, return its mirror code point using the index in m
                return get_mirror_code_point(mirrors[(m >> UBIDI_MIRROR_INDEX_SHIFT) as usize]);
            } else if c < c2 {
                break;
            }
        }
        // c not found, return it itself
        c
    } else {
        c + delta
    }
}

/// The mirror image of `c`, or `c` itself (ICU's `u_charMirror`).
#[must_use]
pub(crate) fn ubidi_get_mirror(c: i32) -> i32 {
    let props = trie_get16(c);
    get_mirror(c, props)
}

#[cfg(test)]
#[must_use]
pub(crate) fn ubidi_get_paired_bracket_type(c: i32) -> u32 {
    let props = trie_get16(c);
    u32::from((props & UBIDI_BPT_MASK) >> UBIDI_BPT_SHIFT)
}

/// The bracket paired with `c`, or `c` itself (ICU's
/// `u_getBidiPairedBracket`).
#[cfg(test)]
#[must_use]
pub(crate) fn ubidi_get_paired_bracket(c: i32) -> i32 {
    let props = trie_get16(c);
    if (props & UBIDI_BPT_MASK) == 0 { c } else { get_mirror(c, props) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties() {
        // L, R, AL, EN, AN, WS, ON, NSM, B
        assert_eq!(ubidi_get_class('a' as i32), 0);
        assert_eq!(ubidi_get_class(0x05D0), 1);
        assert_eq!(ubidi_get_class(0x0627), 13);
        assert_eq!(ubidi_get_class('1' as i32), 2);
        assert_eq!(ubidi_get_class(0x0661), 5);
        assert_eq!(ubidi_get_class(' ' as i32), 9);
        assert_eq!(ubidi_get_class('!' as i32), 10);
        assert_eq!(ubidi_get_class(0x0301), 17);
        assert_eq!(ubidi_get_class('\n' as i32), 7);
        // A supplementary RTL letter (Adlam) and an unassigned code point in
        // the Hebrew block (default R).
        assert_eq!(ubidi_get_class(0x1E900), 1);
        assert_eq!(ubidi_get_class(0x05FF), 1);
        assert_eq!(ubidi_get_mirror('(' as i32), ')' as i32);
        assert_eq!(ubidi_get_mirror('a' as i32), 'a' as i32);
        assert_eq!(ubidi_get_paired_bracket('[' as i32), ']' as i32);
        assert_eq!(ubidi_get_paired_bracket_type('[' as i32), U_BPT_OPEN);
        assert_eq!(ubidi_get_paired_bracket_type(']' as i32), U_BPT_CLOSE);
        assert_eq!(ubidi_get_paired_bracket_type('<' as i32), U_BPT_NONE);
        assert_eq!(ubidi_get_mirror('<' as i32), '>' as i32);
    }
}
