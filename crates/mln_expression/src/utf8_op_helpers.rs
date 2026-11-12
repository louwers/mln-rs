// Ports src/mln/style/expression/utf8_op_helpers.hpp and
// src/mln/style/expression/utf8_op_helpers.cpp.

fn is_continuation_byte(b: u8) -> bool {
    (b & 0xC0) == 0x80
}

/// Get the number of unicode characters of an utf-8 string.
/// Precondition: the string must be valid utf-8 (otherwise, the result may be
/// incorrect).
#[must_use]
pub fn unicode_length_on_validated_utf8(s: &str) -> usize {
    s.bytes().filter(|b| !is_continuation_byte(*b)).count()
}

/// Get the byte offset of a unicode character offset in a utf-8 string: the
/// byte offset of the (start of the) unicode character with the requested
/// offset, or `s.len()` if `char_offset` is larger than the number of
/// unicode characters or otherwise invalid.
#[must_use]
pub fn get_unicode_character_offset_on_validated_utf8(s: &str, char_offset: usize) -> usize {
    let mut cur_char = 0;
    for (i, b) in s.bytes().enumerate() {
        if !is_continuation_byte(b) {
            if cur_char == char_offset {
                return i;
            }
            cur_char += 1;
        }
    }
    s.len()
}
