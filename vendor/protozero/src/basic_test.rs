// Ports the reader cases of vendor/protozero/test/unit/test_basic.cpp, plus
// reader behaviour the C++ tests in test/t/* cover (packed fields, nested
// messages, skipping, truncated data).

use crate::test_util::Writer;
use crate::{Exception, PbfReader, PbfWireType, add_varint_to_buffer};

#[test]
fn default_constructed_pbf_reader_is_okay() {
    let mut item = PbfReader::default();
    assert_eq!(item.length(), 0);
    assert!(!item.as_bool());
    assert_eq!(item.next_field(), Ok(false));
}

#[test]
fn empty_buffer_in_pbf_reader_is_okay() {
    let buffer: Vec<u8> = Vec::new();
    let mut item = PbfReader::new(&buffer);
    assert_eq!(item.length(), 0);
    assert!(!item.as_bool());
    assert_eq!(item.next_field(), Ok(false));
}

#[test]
fn check_every_possible_value_for_single_byte_in_buffer() {
    for i in 0..=255u8 {
        let buffer = [i];
        let mut item = PbfReader::new(&buffer);
        assert_eq!(item.length(), 1);
        assert!(item.as_bool());
        // REQUIRE_THROWS((item.next_field(), item.skip()))
        let r = item.next_field().and_then(|_| item.skip());
        assert!(r.is_err(), "byte {i}");
    }
}

#[test]
fn next_should_throw_when_illegal_wire_type_is_encountered() {
    let buffer = [(1u8 << 3) | 7];
    let mut item = PbfReader::new(&buffer);
    assert_eq!(item.next_field(), Err(Exception::UnknownPbfWireType));
    // The tag and wire type are stored before the check, as in C++.
    assert_eq!(item.tag(), 1);
    assert_eq!(item.wire_type(), PbfWireType(7));
}

fn reader_for_key(key: u64) -> Vec<u8> {
    let mut data = Vec::new();
    add_varint_to_buffer(&mut data, key);
    data
}

#[test]
fn next_should_throw_when_illegal_tag_0_is_encountered() {
    let data = reader_for_key(1);
    assert_eq!(PbfReader::new(&data).next_field(), Err(Exception::InvalidTag));
}

#[test]
fn next_should_throw_when_illegal_tag_19000_is_encountered() {
    let data = reader_for_key(19000 << 3 | 1);
    assert_eq!(PbfReader::new(&data).next_field(), Err(Exception::InvalidTag));
}

#[test]
fn next_should_throw_when_illegal_tag_19001_is_encountered() {
    let data = reader_for_key(19001 << 3 | 1);
    assert_eq!(PbfReader::new(&data).next_field(), Err(Exception::InvalidTag));
}

#[test]
fn next_should_throw_when_illegal_tag_19999_is_encountered() {
    let data = reader_for_key(19999 << 3 | 1);
    assert_eq!(PbfReader::new(&data).next_field(), Err(Exception::InvalidTag));
}

#[test]
fn next_works_when_legal_tag_1_is_encountered() {
    let data = reader_for_key(1 << 3 | 1);
    assert_eq!(PbfReader::new(&data).next_field(), Ok(true));
}

#[test]
fn next_works_when_legal_tag_18999_is_encountered() {
    let data = reader_for_key(18999 << 3 | 1);
    assert_eq!(PbfReader::new(&data).next_field(), Ok(true));
}

#[test]
fn next_works_when_legal_tag_20000_is_encountered() {
    let data = reader_for_key(20000 << 3 | 1);
    assert_eq!(PbfReader::new(&data).next_field(), Ok(true));
}

#[test]
fn next_works_when_legal_tag_1_29_minus_1_is_encountered() {
    let data = reader_for_key(((1u64 << 29) - 1) << 3 | 1);
    assert_eq!(PbfReader::new(&data).next_field(), Ok(true));
}

#[test]
fn key_varint_is_truncated_to_32_bits() {
    // get_varint<uint32_t>: bits above 32 are dropped, so this is tag 1.
    let data = reader_for_key((1u64 << 40) | (1 << 3));
    let mut item = PbfReader::new(&data);
    assert_eq!(item.next_field(), Ok(true));
    assert_eq!(item.tag(), 1);
    assert_eq!(item.wire_type(), PbfWireType::Varint);
}

#[test]
fn scalar_fields_and_skip() {
    let mut w = Writer::default();
    w.add_int32(1, -17);
    w.add_sint32(2, -17);
    w.add_uint64(3, 1 << 40);
    w.add_fixed32(4, 0xdead_beef);
    w.add_fixed64(5, 0x0123_4567_89ab_cdef);
    w.add_bytes(6, b"hello");
    w.add_sint64(7, i64::MIN);
    w.add_uint32(8, 1);

    let mut item = PbfReader::new(&w.buffer);
    assert!(item.next_field().unwrap());
    assert_eq!(item.tag_and_type(), 1 << 3);
    assert_eq!(item.get_int32(), Ok(-17));
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_sint32(), Ok(-17));
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_uint64(), Ok(1 << 40));
    assert!(item.next_field().unwrap());
    assert!(item.has_wire_type(PbfWireType::Fixed32));
    assert_eq!(item.get_fixed32(), Ok(0xdead_beef));
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_sfixed64(), Ok(0x0123_4567_89ab_cdef));
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_string(), Ok("hello".to_owned()));
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_sint64(), Ok(i64::MIN));
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_bool(), Ok(true));
    assert_eq!(item.next_field(), Ok(false));

    let mut item = PbfReader::new(&w.buffer);
    let mut tags = Vec::new();
    while item.next_field().unwrap() {
        tags.push(item.tag());
        item.skip().unwrap();
    }
    assert_eq!(tags, vec![1, 2, 3, 4, 5, 6, 7, 8]);

    let mut item = PbfReader::new(&w.buffer);
    assert_eq!(item.next_tag(6), Ok(true));
    assert_eq!(item.get_view(), Ok(&b"hello"[..]));
    assert_eq!(item.next_tag(6), Ok(false));

    let mut item = PbfReader::new(&w.buffer);
    assert_eq!(item.next_tag_type(6, PbfWireType::Varint), Ok(false));
}

#[test]
fn get_bool_reads_the_first_byte() {
    // A non-canonical zero (80 00) reads as true: C++ checks m_data[0] != 0.
    let data = [1 << 3, 0x80, 0x00];
    let mut item = PbfReader::new(&data);
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_bool(), Ok(true));
    // Truncated bool.
    let data = [1 << 3];
    let mut item = PbfReader::new(&data);
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_bool(), Err(Exception::EndOfBuffer));
}

#[test]
fn truncated_fields() {
    let mut w = Writer::default();
    w.add_bytes(1, b"hello");
    let mut short = w.buffer.clone();
    short.pop();
    let mut item = PbfReader::new(&short);
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_view(), Err(Exception::EndOfBuffer));

    let mut item = PbfReader::new(&short);
    assert!(item.next_field().unwrap());
    assert_eq!(item.skip(), Err(Exception::EndOfBuffer));

    let mut w = Writer::default();
    w.add_fixed64(1, 7);
    let short = &w.buffer[..w.buffer.len() - 1];
    let mut item = PbfReader::new(short);
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_fixed64(), Err(Exception::EndOfBuffer));
}

#[test]
fn length_is_truncated_to_32_bits() {
    // get_length is get_varint<uint32_t>: a length of 2^32 + 1 is 1.
    let mut data = vec![1 << 3 | 2];
    add_varint_to_buffer(&mut data, (1u64 << 32) + 1);
    data.push(b'x');
    let mut item = PbfReader::new(&data);
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_view(), Ok(&b"x"[..]));
}

#[test]
fn packed_fields() {
    let mut w = Writer::default();
    w.add_packed_uint64(1, &[1, 300, u64::MAX]);
    let mut item = PbfReader::new(&w.buffer);
    assert!(item.next_field().unwrap());
    let range = item.get_packed_uint64().unwrap();
    assert_eq!(range.size(), 3);
    assert_eq!(range.front(), Ok(1));
    let values: Result<Vec<u64>, _> = range.into_iter().collect();
    assert_eq!(values, Ok(vec![1, 300, u64::MAX]));

    let mut item = PbfReader::new(&w.buffer);
    assert!(item.next_field().unwrap());
    let range = item.get_packed_uint32().unwrap();
    let values: Result<Vec<u32>, _> = range.into_iter().collect();
    assert_eq!(values, Ok(vec![1, 300, u32::MAX]));

    // Packed fixed with a bad length.
    let mut w = Writer::default();
    w.add_bytes(1, &[0, 0, 0, 0, 0]);
    let mut item = PbfReader::new(&w.buffer);
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_packed_fixed32().map(|r| r.size()), Err(Exception::InvalidLength));
    let mut item = PbfReader::new(&w.buffer);
    assert!(item.next_field().unwrap());
    let r = item.get_packed_sfixed32().err();
    assert_eq!(r, Some(Exception::InvalidLength));

    let mut w = Writer::default();
    w.add_bytes(1, &1.5f32.to_le_bytes());
    let mut item = PbfReader::new(&w.buffer);
    assert!(item.next_field().unwrap());
    assert_eq!(item.get_packed_float().unwrap().into_iter().collect::<Vec<_>>(), vec![1.5f32]);

    // Packed sint32.
    let mut w = Writer::default();
    w.add_packed_uint64(1, &[u64::from(crate::encode_zigzag32(-5)), crate::encode_zigzag64(i64::MIN)]);
    let mut item = PbfReader::new(&w.buffer);
    assert!(item.next_field().unwrap());
    let values: Result<Vec<i64>, _> = item.get_packed_sint64().unwrap().into_iter().collect();
    assert_eq!(values, Ok(vec![-5, i64::MIN]));
}

#[test]
fn nested_messages() {
    let mut inner = Writer::default();
    inner.add_uint32(1, 42);
    let mut outer = Writer::default();
    outer.add_bytes(3, &inner.buffer);
    outer.add_bytes(3, &inner.buffer);

    let mut item = PbfReader::new(&outer.buffer);
    let mut n = 0;
    while item.next_tag(3).unwrap() {
        let mut msg = item.get_message().unwrap();
        assert!(msg.next_field().unwrap());
        assert_eq!(msg.get_uint32(), Ok(42));
        assert_eq!(msg.next_field(), Ok(false));
        n += 1;
    }
    assert_eq!(n, 2);
}

#[cfg(debug_assertions)]
#[test]
fn debug_builds_reset_the_tag_after_reading() {
    let mut w = Writer::default();
    w.add_bytes(1, b"x");
    let mut item = PbfReader::new(&w.buffer);
    assert!(item.next_field().unwrap());
    assert_eq!(item.tag(), 1);
    item.get_view().unwrap();
    assert_eq!(item.tag(), 0);
}
