// Ports include/libnu/utf8_internal.h and the UTF-8 reader/writer of
// src/libnu/utf8.c (nu_utf8_read, nu_utf8_write).
//
// The C functions walk raw `const char*`; here a position is an index into
// a byte slice, and reading at or past the end yields the 0 terminator, as
// reading a C string's NUL does.

#[inline]
fn byte(s: &[u8], i: usize) -> u32 {
    u32::from(s.get(i).copied().unwrap_or(0))
}

#[inline]
fn utf8_2b(s: &[u8], i: usize) -> u32 {
    let (b0, b1) = (byte(s, i), byte(s, i + 1));
    ((b0 & 0x1C) << 6) | (((b0 & 0x03) << 6) | (b1 & 0x3F))
}

#[inline]
fn utf8_3b(s: &[u8], i: usize) -> u32 {
    let (b0, b1, b2) = (byte(s, i), byte(s, i + 1), byte(s, i + 2));
    (((b0 & 0x0F) << 12) | ((b1 & 0x3C) << 6)) | (((b1 & 0x03) << 6) | (b2 & 0x3F))
}

#[inline]
fn utf8_4b(s: &[u8], i: usize) -> u32 {
    let (b0, b1, b2, b3) = (byte(s, i), byte(s, i + 1), byte(s, i + 2), byte(s, i + 3));
    (((b0 & 0x07) << 18) | ((b1 & 0x30) << 12))
        | (((b1 & 0x0F) << 12) | ((b2 & 0x3C) << 6))
        | (((b2 & 0x03) << 6) | (b3 & 0x3F))
}

/// `nu_utf8_read`: decodes the codepoint at `i`, returns the position of the
/// next one and the codepoint.
#[inline]
#[must_use]
pub fn nu_utf8_read(s: &[u8], i: usize) -> (usize, u32) {
    let c = byte(s, i);
    if c >= 0x80 {
        if c < 0xE0 {
            (i + 2, utf8_2b(s, i))
        } else if c < 0xF0 {
            (i + 3, utf8_3b(s, i))
        } else {
            (i + 4, utf8_4b(s, i))
        }
    } else {
        (i + 1, c)
    }
}

fn utf8_codepoint_length(codepoint: u32) -> usize {
    if codepoint < 128 {
        return 1;
    }
    if codepoint < 0x0800 {
        return 2;
    }
    if codepoint < 0x10000 {
        return 3;
    }
    4 // de facto max length in UTF-8
}

/// `nu_utf8_write`: appends the encoding of `unicode` to `out`.
pub fn nu_utf8_write(unicode: u32, out: &mut Vec<u8>) {
    let c = unicode;
    match utf8_codepoint_length(unicode) {
        1 => out.push(c as u8),
        2 => {
            // b2_utf8
            out.push((0xC0 | (c & 0xFF00) >> 6 | (c & 0xFF) >> 6) as u8);
            out.push((0x80 | (c & 0x3F)) as u8);
        }
        3 => {
            // b3_utf8
            out.push((0xE0 | (c & 0xF000) >> 12) as u8);
            out.push((0x80 | (c & 0x0F00) >> 6 | (c & 0xC0) >> 6) as u8);
            out.push((0x80 | (c & 0x3F)) as u8);
        }
        _ => {
            // b4_utf8 (len == 4). Quirk kept from the C: bit 12 goes into
            // the third byte (mask 0x001F00) instead of the second, which
            // yields invalid UTF-8 when it is set.
            out.push((0xF0 | ((c & 0x001C_0000) >> 18)) as u8);
            out.push((0x80 | (c & 0x0003_0000) >> 12 | (c & 0x0000_E000) >> 12) as u8);
            out.push((0x80 | (c & 0x0000_1F00) >> 6 | (c & 0x0000_00E0) >> 6) as u8);
            out.push((0x80 | (c & 0x3F)) as u8);
        }
    }
}
