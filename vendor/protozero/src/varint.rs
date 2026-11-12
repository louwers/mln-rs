// Ports vendor/protozero/include/protozero/varint.hpp.
//
// The C++ functions take `const char** data, const char* end`; here the
// cursor is a `&mut &[u8]` spanning [data, end), and "advance the pointer"
// is re-slicing it. On an error the cursor is left unchanged (the C++
// strong exception guarantee).

use crate::exception::Exception;

/// `max_varint_length`: the maximum length of a 64 bit varint
/// (`sizeof(uint64_t) * 8 / 7 + 1`).
pub const MAX_VARINT_LENGTH: i8 = (u64::BITS / 7 + 1) as i8;

mod detail {
    use super::MAX_VARINT_LENGTH;
    use crate::exception::Exception;

    /// `detail::decode_varint_impl`, from folly's Varint.h. The bytes are
    /// read as `int8_t` as in C++: a negative byte has the continuation bit.
    pub(super) fn decode_varint_impl(data: &mut &[u8]) -> Result<u64, Exception> {
        let begin: &[u8] = data;
        let mut p = 0usize;
        let mut val: u64 = 0;

        if begin.len() >= MAX_VARINT_LENGTH as usize {
            // fast path: at most 10 bytes, the 10th contributes only bit 63.
            let mut done = false;
            for i in 0..10u32 {
                let b = i64::from(begin[p] as i8);
                p += 1;
                if i < 9 {
                    val |= ((b as u64) & 0x7f) << (7 * i);
                } else {
                    val |= ((b as u64) & 0x01) << 63;
                }
                if b >= 0 {
                    done = true;
                    break;
                }
            }
            if !done {
                return Err(Exception::VarintTooLong);
            }
        } else {
            let mut shift = 0u32;
            while p != begin.len() && (begin[p] as i8) < 0 {
                val |= (i64::from(begin[p] as i8) as u64 & 0x7f) << shift;
                p += 1;
                shift += 7;
            }
            if p == begin.len() {
                return Err(Exception::EndOfBuffer);
            }
            val |= (i64::from(begin[p] as i8) as u64) << shift;
            p += 1;
        }

        *data = &begin[p..];
        Ok(val)
    }
}

/// Decode a 64 bit varint.
///
/// Strong exception guarantee: on an error `data` is not changed.
///
/// Errors: `VarintTooLong` if the varint is longer than the maximum that
/// fits in a 64 bit int (only detected when at least 10 bytes are left),
/// `EndOfBuffer` if the end of the buffer was reached before the end of the
/// varint.
pub fn decode_varint(data: &mut &[u8]) -> Result<u64, Exception> {
    // If this is a one-byte varint, decode it here.
    if let Some(&b) = data.first()
        && b & 0x80 == 0
    {
        *data = &data[1..];
        return Ok(u64::from(b));
    }
    // If this varint is more than one byte, defer to complete implementation.
    detail::decode_varint_impl(data)
}

/// Skip over a varint.
///
/// Strong exception guarantee: on an error `data` is not changed.
///
/// Errors: `VarintTooLong` if there are 10 or more bytes with the
/// continuation bit (checked first), `EndOfBuffer` if the end of the buffer
/// was reached before the end of the varint.
pub fn skip_varint(data: &mut &[u8]) -> Result<(), Exception> {
    let begin: &[u8] = data;
    let mut p = 0usize;

    while p != begin.len() && (begin[p] as i8) < 0 {
        p += 1;
    }

    if p >= MAX_VARINT_LENGTH as usize {
        return Err(Exception::VarintTooLong);
    }

    if p == begin.len() {
        return Err(Exception::EndOfBuffer);
    }

    p += 1;

    *data = &begin[p..];
    Ok(())
}

/// `add_varint_to_buffer(TBuffer*, uint64_t)`: varint encode a 64 bit
/// integer at the end of `buffer`.
pub fn add_varint_to_buffer(buffer: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        buffer.push(((value & 0x7f) | 0x80) as u8);
        value >>= 7;
    }
    buffer.push(value as u8);
}

/// `length_of_varint`: the number of bytes the varint of `value` would have.
#[must_use]
pub fn length_of_varint(mut value: u64) -> i32 {
    let mut n = 1;
    while value >= 0x80 {
        value >>= 7;
        n += 1;
    }
    n
}

/// ZigZag encodes a 32 bit integer.
#[must_use]
pub const fn encode_zigzag32(value: i32) -> u32 {
    ((value as u32) << 1) ^ ((-(((value as u32) >> 31) as i32)) as u32)
}

/// ZigZag encodes a 64 bit integer.
#[must_use]
pub const fn encode_zigzag64(value: i64) -> u64 {
    ((value as u64) << 1) ^ ((-(((value as u64) >> 63) as i64)) as u64)
}

/// Decodes a 32 bit ZigZag-encoded integer.
#[must_use]
pub const fn decode_zigzag32(value: u32) -> i32 {
    ((value >> 1) ^ ((-((value & 1) as i32)) as u32)) as i32
}

/// Decodes a 64 bit ZigZag-encoded integer.
#[must_use]
pub const fn decode_zigzag64(value: u64) -> i64 {
    ((value >> 1) ^ ((-((value & 1) as i64)) as u64)) as i64
}
