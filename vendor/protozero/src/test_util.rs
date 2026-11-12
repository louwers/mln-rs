// Test-only protobuf encoding, standing in for the parts of
// protozero::pbf_writer (basic_pbf_writer.hpp) that the ported reader tests
// use. The writer itself is not ported (the C++ core does not use it).

use crate::types::PbfWireType;
use crate::varint::{add_varint_to_buffer, encode_zigzag32, encode_zigzag64};

#[derive(Default)]
pub struct Writer {
    pub buffer: Vec<u8>,
}

impl Writer {
    fn key(&mut self, tag: u32, wire_type: PbfWireType) {
        add_varint_to_buffer(&mut self.buffer, u64::from((tag << 3) | wire_type.0));
    }

    pub fn add_uint64(&mut self, tag: u32, v: u64) {
        self.key(tag, PbfWireType::Varint);
        add_varint_to_buffer(&mut self.buffer, v);
    }

    pub fn add_uint32(&mut self, tag: u32, v: u32) {
        self.add_uint64(tag, u64::from(v));
    }

    /// `pbf_writer::add_int32` writes the value sign-extended to 64 bits.
    pub fn add_int32(&mut self, tag: u32, v: i32) {
        self.add_uint64(tag, i64::from(v) as u64);
    }

    pub fn add_int64(&mut self, tag: u32, v: i64) {
        self.add_uint64(tag, v as u64);
    }

    pub fn add_sint32(&mut self, tag: u32, v: i32) {
        self.add_uint64(tag, u64::from(encode_zigzag32(v)));
    }

    pub fn add_sint64(&mut self, tag: u32, v: i64) {
        self.add_uint64(tag, encode_zigzag64(v));
    }

    pub fn add_fixed32(&mut self, tag: u32, v: u32) {
        self.key(tag, PbfWireType::Fixed32);
        self.buffer.extend_from_slice(&v.to_le_bytes());
    }

    pub fn add_fixed64(&mut self, tag: u32, v: u64) {
        self.key(tag, PbfWireType::Fixed64);
        self.buffer.extend_from_slice(&v.to_le_bytes());
    }

    pub fn add_bytes(&mut self, tag: u32, v: &[u8]) {
        self.key(tag, PbfWireType::LengthDelimited);
        add_varint_to_buffer(&mut self.buffer, v.len() as u64);
        self.buffer.extend_from_slice(v);
    }

    pub fn add_packed_uint64(&mut self, tag: u32, values: &[u64]) {
        let mut packed = Vec::new();
        for &v in values {
            add_varint_to_buffer(&mut packed, v);
        }
        self.add_bytes(tag, &packed);
    }
}
