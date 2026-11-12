// Ports vendor/protozero/test/unit/test_varint.cpp and test_zigzag.cpp.

use crate::test_util::Writer;
use crate::*;

#[test]
fn max_varint_length() {
    assert_eq!(MAX_VARINT_LENGTH, 10);
}

#[test]
fn varint() {
    // encode/decode int32
    for skip in [false, true] {
        let mut w = Writer::default();
        w.add_int32(1, 17);
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        if skip {
            item.skip().unwrap();
        } else {
            assert_eq!(item.get_int32(), Ok(17));
        }
        assert_eq!(item.next_field(), Ok(false));
    }

    // encode/decode uint32
    for skip in [false, true] {
        let mut w = Writer::default();
        w.add_uint32(1, 17);
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        if skip {
            item.skip().unwrap();
        } else {
            assert_eq!(item.get_uint32(), Ok(17));
        }
        assert_eq!(item.next_field(), Ok(false));
    }

    // encode/decode uint64
    for skip in [false, true] {
        let mut w = Writer::default();
        w.add_uint64(1, 1 << 40);
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        if skip {
            item.skip().unwrap();
        } else {
            assert_eq!(item.get_uint64(), Ok(1 << 40));
        }
        assert_eq!(item.next_field(), Ok(false));
    }

    // short buffer while parsing varint
    {
        let mut w = Writer::default();
        w.add_uint64(1, 1 << 40);
        w.buffer.pop(); // "remove" last byte from buffer
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        assert_eq!(item.get_uint64(), Err(Exception::EndOfBuffer));
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        assert_eq!(item.skip(), Err(Exception::EndOfBuffer));
    }

    // data corruption in buffer while parsing varint
    {
        let mut w = Writer::default();
        w.add_uint64(1, 1 << 20);
        let last = w.buffer.len() - 1;
        w.buffer[last] = w.buffer[last].wrapping_add(0x80); // pretend the varint goes on
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        assert_eq!(item.get_uint64(), Err(Exception::EndOfBuffer));
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        assert_eq!(item.skip(), Err(Exception::EndOfBuffer));
    }

    // data corruption in buffer while parsing varint (max length varint)
    {
        let mut w = Writer::default();
        w.add_uint64(1, u64::MAX);
        let last = w.buffer.len() - 1;
        w.buffer[last] = w.buffer[last].wrapping_add(0x80); // pretend the varint goes on
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        assert_eq!(item.get_uint64(), Err(Exception::VarintTooLong));
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        assert_eq!(item.skip(), Err(Exception::VarintTooLong));
    }
}

#[test]
fn ten_byte_varint() {
    let mut w = Writer::default();
    w.add_uint64(1, 1);
    *w.buffer.last_mut().unwrap() = 0xff;
    for _ in 0..9 {
        w.buffer.push(0xff);
    }
    w.buffer.push(0x02);

    let mut item = PbfReader::new(&w.buffer);
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_uint64(), Err(Exception::VarintTooLong));
}

#[test]
fn lots_of_varints_back_and_forth() {
    for n in 0..70000u32 {
        let mut w = Writer::default();
        w.add_uint32(1, n);
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        assert_eq!(item.get_uint32(), Ok(n));
        assert_eq!(item.next_field(), Ok(false));
    }

    for n in -70000..70000i32 {
        let mut w = Writer::default();
        w.add_int32(1, n);
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        assert_eq!(item.get_int32(), Ok(n));
        assert_eq!(item.next_field(), Ok(false));
    }

    for n in -70000..70000i32 {
        let mut w = Writer::default();
        w.add_sint32(1, n);
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        assert_eq!(item.get_sint32(), Ok(n));
        assert_eq!(item.next_field(), Ok(false));
    }

    for i in 0..63 {
        for n in [(1u64 << i) as i64, -((1u64 << i) as i64)] {
            let mut w = Writer::default();
            w.add_int64(1, n);
            let mut item = PbfReader::new(&w.buffer);
            assert!(item.next_field().unwrap());
            assert_eq!(item.get_int64(), Ok(n));
            assert_eq!(item.next_field(), Ok(false));
        }
    }

    for i in 0..64 {
        let n = 1u64 << i;
        let mut w = Writer::default();
        w.add_uint64(1, n);
        let mut item = PbfReader::new(&w.buffer);
        assert!(item.next_field().unwrap());
        assert_eq!(item.get_uint64(), Ok(n));
        assert_eq!(item.next_field(), Ok(false));
    }
}

#[test]
fn skip_varint_with_empty_buffer_throws() {
    let mut buffer: &[u8] = b"";
    assert_eq!(skip_varint(&mut buffer), Err(Exception::EndOfBuffer));
}

#[test]
fn call_skip_varint_with_every_possible_value_for_single_byte_in_buffer() {
    for i in 0..=127u8 {
        let data = [i];
        let mut b: &[u8] = &data;
        skip_varint(&mut b).unwrap();
        assert_eq!(b, []);
    }
    for i in 128..=255u8 {
        let data = [i];
        let mut b: &[u8] = &data;
        assert_eq!(skip_varint(&mut b), Err(Exception::EndOfBuffer));
        assert_eq!(b.len(), 1); // strong exception guarantee
    }
}

#[test]
fn decode_varint_with_empty_buffer_throws() {
    let mut buffer: &[u8] = b"";
    assert_eq!(decode_varint(&mut buffer), Err(Exception::EndOfBuffer));
}

#[test]
fn call_decode_varint_with_every_possible_value_for_single_byte_in_buffer() {
    for i in 0..=127u8 {
        assert_eq!(length_of_varint(u64::from(i)), 1);
        let data = [i];
        let mut b: &[u8] = &data;
        assert_eq!(decode_varint(&mut b), Ok(u64::from(i)));
        assert_eq!(b, []);
    }
    for i in 128..=255u8 {
        assert_eq!(length_of_varint(u64::from(i)), 2);
        let data = [i];
        let mut b: &[u8] = &data;
        assert_eq!(decode_varint(&mut b), Err(Exception::EndOfBuffer));
    }
}

#[test]
fn check_lengths_of_varint() {
    assert_eq!(length_of_varint(0), 1);
    assert_eq!(length_of_varint(127), 1);
    assert_eq!(length_of_varint(128), 2);
    assert_eq!(length_of_varint(16383), 2);
    assert_eq!(length_of_varint(16384), 3);
    assert_eq!(length_of_varint(2_097_151), 3);
    assert_eq!(length_of_varint(2_097_152), 4);
    assert_eq!(length_of_varint(0xffff_ffff), 5);
    assert_eq!(length_of_varint(0xffff_ffff_ffff_ffff), 10);
}

#[test]
fn fast_and_slow_paths_differ_for_overlong_varints() {
    // Fast path (>= 10 bytes left): the 10th byte contributes only bit 0.
    let data = [0xffu8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];
    let mut b: &[u8] = &data;
    assert_eq!(decode_varint(&mut b), Ok(u64::MAX));
    // skip_varint accepts it too (9 continuation bytes).
    let mut b: &[u8] = &data;
    assert_eq!(skip_varint(&mut b), Ok(()));
    // 10 continuation bytes: too long, even at the end of the buffer.
    let data = [0x80u8; 10];
    let mut b: &[u8] = &data;
    assert_eq!(skip_varint(&mut b), Err(Exception::VarintTooLong));
    let mut b: &[u8] = &data;
    assert_eq!(decode_varint(&mut b), Err(Exception::VarintTooLong));
    // Slow path (< 10 bytes left): runs into the end.
    let data = [0x80u8; 9];
    let mut b: &[u8] = &data;
    assert_eq!(decode_varint(&mut b), Err(Exception::EndOfBuffer));
}

#[test]
fn zigzag_encode_some_32_bit_values() {
    assert_eq!(encode_zigzag32(0), 0);
    assert_eq!(encode_zigzag32(-1), 1);
    assert_eq!(encode_zigzag32(1), 2);
    assert_eq!(encode_zigzag32(-2), 3);
    assert_eq!(encode_zigzag32(2), 4);
    assert_eq!(encode_zigzag32(i32::MAX), 2 * i32::MAX as u32);
    assert_eq!(encode_zigzag32(-i32::MAX), 2 * i32::MAX as u32 - 1);
}

#[test]
fn zigzag_encode_some_64_bit_values() {
    assert_eq!(encode_zigzag64(0), 0);
    assert_eq!(encode_zigzag64(-1), 1);
    assert_eq!(encode_zigzag64(1), 2);
    assert_eq!(encode_zigzag64(-2), 3);
    assert_eq!(encode_zigzag64(2), 4);
    assert_eq!(encode_zigzag64(i64::MAX), 2 * i64::MAX as u64);
    assert_eq!(encode_zigzag64(-i64::MAX), 2 * i64::MAX as u64 - 1);
}

#[test]
fn zigzag_decode_some_values() {
    assert_eq!(decode_zigzag32(0), 0);
    assert_eq!(decode_zigzag32(1), -1);
    assert_eq!(decode_zigzag32(2), 1);
    assert_eq!(decode_zigzag32(3), -2);
    assert_eq!(decode_zigzag32(4), 2);
    assert_eq!(decode_zigzag32(0xffff_fffe), 0x7fff_ffff);
    assert_eq!(decode_zigzag32(0xffff_fffd), -0x7fff_ffff);
    assert_eq!(decode_zigzag32(2 * i32::MAX as u32), i32::MAX);
    assert_eq!(decode_zigzag64(0), 0);
    assert_eq!(decode_zigzag64(1), -1);
    assert_eq!(decode_zigzag64(2), 1);
    assert_eq!(decode_zigzag64(3), -2);
    assert_eq!(decode_zigzag64(4), 2);
    assert_eq!(decode_zigzag64(0xffff_ffff_ffff_fffe), 0x7fff_ffff_ffff_ffff);
    assert_eq!(decode_zigzag64(0xffff_ffff_ffff_fffd), -0x7fff_ffff_ffff_ffff);
    assert_eq!(decode_zigzag64(2 * i64::MAX as u64), i64::MAX);
    assert_eq!(decode_zigzag64(2 * i64::MAX as u64 - 1), -i64::MAX);
}

#[test]
fn zigzag_and_back() {
    let zz32 = |v: i32| decode_zigzag32(encode_zigzag32(v));
    let zz64 = |v: i64| decode_zigzag64(encode_zigzag64(v));
    for v in [0, 1, -1, i32::MAX, i32::MIN] {
        assert_eq!(zz32(v), v);
    }
    for v in [0, 1, -1, i64::MAX, i64::MIN] {
        assert_eq!(zz64(v), v);
    }
}
