// Ports vendor/protozero/include/protozero/pbf_reader.hpp.

use crate::data_view::DataView;
use crate::exception::Exception;
use crate::iterators::{
    ConstFixedIterator, ConstSvarintIterator, ConstVarintIterator, FixedValue, IteratorRange, SvarintValue, VarintValue,
};
use crate::types::{PbfLengthType, PbfTagType, PbfWireType, tag_and_type};
use crate::varint::{decode_varint, decode_zigzag64, skip_varint};

/// `protozero::pbf_reader`: reads a protocol buffer message (or part of
/// one) from a byte buffer it does not own.
///
/// Usage: call [`next_field`](Self::next_field) (C++ `next`) until it returns `false`;
/// for each field look at [`tag`](Self::tag) (and
/// [`wire_type`](Self::wire_type)) and call one `get_*` function or
/// [`skip`](Self::skip). Every function that the C++ lets throw returns
/// `Result<_, Exception>` with the same condition.
///
/// `pbf_reader` messages can be copied trivially (`Copy`).
#[derive(Clone, Copy, Debug)]
pub struct PbfReader<'a> {
    /// [`m_data`, `m_end)`: the next unread data up to one past the end.
    data: &'a [u8],
    /// The wire type of the current field.
    wire_type: PbfWireType,
    /// The tag of the current field.
    tag: PbfTagType,
}

impl Default for PbfReader<'_> {
    /// `pbf_reader() noexcept = default`: a reader with no data.
    fn default() -> Self {
        PbfReader { data: &[], wire_type: PbfWireType::Unknown, tag: 0 }
    }
}

impl<'a> PbfReader<'a> {
    /// Construct a `pbf_reader` message from a `data_view` / `std::string` /
    /// (pointer, size) pair: all are a byte slice here. The data is not
    /// copied; it must outlive the reader.
    #[must_use]
    pub fn new(data: DataView<'a>) -> Self {
        PbfReader { data, wire_type: PbfWireType::Unknown, tag: 0 }
    }

    fn get_fixed<T: FixedValue>(&mut self) -> Result<T, Exception> {
        let data = self.data;
        self.skip_bytes(T::SIZE as PbfLengthType)?;
        Ok(T::from_bytes(&data[..T::SIZE]))
    }

    fn packed_fixed<T: FixedValue>(&mut self) -> Result<IteratorRange<ConstFixedIterator<'a, T>>, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        let packed = self.get_len_and_skip()?;
        if packed.len() % T::SIZE != 0 {
            return Err(Exception::InvalidLength);
        }
        Ok(IteratorRange::new(ConstFixedIterator::new(packed, 0), ConstFixedIterator::new(packed, packed.len())))
    }

    fn get_varint<T: VarintValue>(&mut self) -> Result<T, Exception> {
        Ok(T::from_varint(decode_varint(&mut self.data)?))
    }

    fn get_svarint<T: SvarintValue>(&mut self) -> Result<T, Exception> {
        // C++: protozero_assert(varint || length_delimited, "not a varint").
        Ok(T::from_zigzag64(decode_zigzag64(decode_varint(&mut self.data)?)))
    }

    fn get_length(&mut self) -> Result<PbfLengthType, Exception> {
        self.get_varint::<PbfLengthType>()
    }

    fn skip_bytes(&mut self, len: PbfLengthType) -> Result<(), Exception> {
        if (self.data.len() as isize) < len as isize {
            return Err(Exception::EndOfBuffer);
        }
        self.data = &self.data[len as usize..];
        // In debug builds reset the tag to zero so that we can detect (some)
        // wrong code (C++: #ifndef NDEBUG).
        #[cfg(debug_assertions)]
        {
            self.tag = 0;
        }
        Ok(())
    }

    /// `get_len_and_skip`, returning the skipped bytes [`m_data` - len, `m_data`).
    fn get_len_and_skip(&mut self) -> Result<&'a [u8], Exception> {
        let len = self.get_length()?;
        let start = self.data;
        self.skip_bytes(len)?;
        Ok(&start[..len as usize])
    }

    fn get_packed_varint<T: VarintValue>(&mut self) -> Result<IteratorRange<ConstVarintIterator<'a, T>>, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        let packed = self.get_len_and_skip()?;
        Ok(IteratorRange::new(ConstVarintIterator::new(packed), ConstVarintIterator::new(&packed[packed.len()..])))
    }

    fn get_packed_svarint<T: SvarintValue>(&mut self) -> Result<IteratorRange<ConstSvarintIterator<'a, T>>, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        let packed = self.get_len_and_skip()?;
        Ok(IteratorRange::new(ConstSvarintIterator::new(packed), ConstSvarintIterator::new(&packed[packed.len()..])))
    }

    /// Swap the contents of this object with the other.
    pub fn swap(&mut self, other: &mut Self) {
        std::mem::swap(self, other);
    }

    /// `operator bool`: in a boolean context the `pbf_reader` class evaluates
    /// to true if there are still fields available and to false if the last
    /// field has been read.
    #[must_use]
    pub fn as_bool(&self) -> bool {
        !self.data.is_empty()
    }

    /// Get a view of the not yet read data.
    #[must_use]
    pub fn data(&self) -> DataView<'a> {
        self.data
    }

    /// Return the length in bytes of the current message. If you have
    /// already called `next_field()` and/or any of the get_*() functions, this will
    /// return the remaining length.
    #[must_use]
    pub fn length(&self) -> usize {
        self.data.len()
    }

    /// Set next field in the message as the current field. Returns `false`
    /// at the end of the data.
    ///
    /// Errors: `InvalidTag` for tag 0 or 19000..=19999,
    /// `UnknownPbfWireType` for wire types 3, 4, 6 and 7 (the tag and wire
    /// type are stored first, as in C++), and the varint errors of the key.
    pub fn next_field(&mut self) -> Result<bool, Exception> {
        if self.data.is_empty() {
            return Ok(false);
        }

        let value = self.get_varint::<u32>()?;
        self.tag = value >> 3;

        // tags 0 and 19000 to 19999 are not allowed as per
        // https://developers.google.com/protocol-buffers/docs/proto#assigning-tags
        if self.tag == 0 || (self.tag >= 19000 && self.tag <= 19999) {
            return Err(Exception::InvalidTag);
        }

        self.wire_type = PbfWireType(value & 0x07);
        match self.wire_type {
            PbfWireType::Varint | PbfWireType::Fixed64 | PbfWireType::LengthDelimited | PbfWireType::Fixed32 => {}
            _ => return Err(Exception::UnknownPbfWireType),
        }

        Ok(true)
    }

    /// `next(pbf_tag_type next_tag)`: set next field with the given tag as
    /// the current field, skipping all other fields.
    pub fn next_tag(&mut self, next_tag: PbfTagType) -> Result<bool, Exception> {
        while self.next_field()? {
            if self.tag == next_tag {
                return Ok(true);
            }
            self.skip()?;
        }
        Ok(false)
    }

    /// `next(pbf_tag_type next_tag, pbf_wire_type type)`: set next field with
    /// the given tag and wire type as the current field.
    pub fn next_tag_type(&mut self, next_tag: PbfTagType, type_: PbfWireType) -> Result<bool, Exception> {
        while self.next_field()? {
            if self.tag == next_tag && self.wire_type == type_ {
                return Ok(true);
            }
            self.skip()?;
        }
        Ok(false)
    }

    /// The tag of the current field. The tag is the field number from the
    /// description in the .proto file. 0 before `next_field()` (and, in debug
    /// builds, after a get_*() that skipped bytes).
    #[must_use]
    pub fn tag(&self) -> PbfTagType {
        self.tag
    }

    /// Get the wire type of the current field.
    #[must_use]
    pub fn wire_type(&self) -> PbfWireType {
        self.wire_type
    }

    /// Get the tag and wire type of the current field in one integer.
    #[must_use]
    pub fn tag_and_type(&self) -> u32 {
        tag_and_type(self.tag(), self.wire_type())
    }

    /// Check the wire type of the current field.
    #[must_use]
    pub fn has_wire_type(&self, type_: PbfWireType) -> bool {
        self.wire_type() == type_
    }

    /// Consume the current field.
    pub fn skip(&mut self) -> Result<(), Exception> {
        debug_assert!(self.tag() != 0, "call next() before calling skip()");
        match self.wire_type() {
            PbfWireType::Varint => skip_varint(&mut self.data)?,
            PbfWireType::Fixed64 => self.skip_bytes(8)?,
            PbfWireType::LengthDelimited => {
                let len = self.get_length()?;
                self.skip_bytes(len)?;
            }
            PbfWireType::Fixed32 => self.skip_bytes(4)?,
            _ => {}
        }
        Ok(())
    }

    // The C++ asserts the wire type in each get_*() ("not a varint", ...);
    // see the crate docs.

    /// Consume and return value of current "bool" field. Quirk: the result
    /// is the first byte != 0, so a non-canonical varint like `80 00` is
    /// true.
    pub fn get_bool(&mut self) -> Result<bool, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        // C++ reads m_data[0] before checking the end; then skip_varint
        // throws for an empty buffer, so the value never escapes.
        let result = self.data.first().is_some_and(|&b| b != 0);
        skip_varint(&mut self.data)?;
        Ok(result)
    }

    /// Consume and return value of current "enum" field.
    pub fn get_enum(&mut self) -> Result<i32, Exception> {
        self.get_varint::<i32>()
    }

    /// Consume and return value of current "int32" varint field.
    pub fn get_int32(&mut self) -> Result<i32, Exception> {
        self.get_varint::<i32>()
    }

    /// Consume and return value of current "sint32" varint field.
    pub fn get_sint32(&mut self) -> Result<i32, Exception> {
        self.get_svarint::<i32>()
    }

    /// Consume and return value of current "uint32" varint field.
    pub fn get_uint32(&mut self) -> Result<u32, Exception> {
        self.get_varint::<u32>()
    }

    /// Consume and return value of current "int64" varint field.
    pub fn get_int64(&mut self) -> Result<i64, Exception> {
        self.get_varint::<i64>()
    }

    /// Consume and return value of current "sint64" varint field.
    pub fn get_sint64(&mut self) -> Result<i64, Exception> {
        self.get_svarint::<i64>()
    }

    /// Consume and return value of current "uint64" varint field.
    pub fn get_uint64(&mut self) -> Result<u64, Exception> {
        self.get_varint::<u64>()
    }

    /// Consume and return value of current "fixed32" field.
    pub fn get_fixed32(&mut self) -> Result<u32, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        self.get_fixed::<u32>()
    }

    /// Consume and return value of current "sfixed32" field.
    pub fn get_sfixed32(&mut self) -> Result<i32, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        self.get_fixed::<i32>()
    }

    /// Consume and return value of current "fixed64" field.
    pub fn get_fixed64(&mut self) -> Result<u64, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        self.get_fixed::<u64>()
    }

    /// Consume and return value of current "sfixed64" field.
    pub fn get_sfixed64(&mut self) -> Result<i64, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        self.get_fixed::<i64>()
    }

    /// Consume and return value of current "float" field.
    pub fn get_float(&mut self) -> Result<f32, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        self.get_fixed::<f32>()
    }

    /// Consume and return value of current "double" field.
    pub fn get_double(&mut self) -> Result<f64, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        self.get_fixed::<f64>()
    }

    /// Consume and return value of current "bytes", "string", or "message"
    /// field, as a view into the buffer.
    pub fn get_view(&mut self) -> Result<DataView<'a>, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        self.get_len_and_skip()
    }

    /// `get_data()`: the same as [`get_view`](Self::get_view) (C++ returns a
    /// `std::pair<const char*, pbf_length_type>`).
    pub fn get_data(&mut self) -> Result<DataView<'a>, Exception> {
        debug_assert!(self.tag() != 0, "call next() before accessing field value");
        self.get_len_and_skip()
    }

    /// Consume and return value of current "bytes" field (a copy).
    pub fn get_bytes(&mut self) -> Result<Vec<u8>, Exception> {
        Ok(self.get_view()?.to_vec())
    }

    /// Consume and return value of current "string" field.
    ///
    /// C++ returns a `std::string` holding the bytes as they are; a Rust
    /// `String` must be UTF-8, so invalid sequences become U+FFFD here
    /// (`String::from_utf8_lossy`). Use [`get_view`](Self::get_view) or
    /// [`get_bytes`](Self::get_bytes) for the exact bytes.
    pub fn get_string(&mut self) -> Result<String, Exception> {
        Ok(String::from_utf8_lossy(self.get_view()?).into_owned())
    }

    /// Consume and return value of current "message" field.
    pub fn get_message(&mut self) -> Result<PbfReader<'a>, Exception> {
        Ok(PbfReader::new(self.get_view()?))
    }

    /// Consume current "repeated packed bool" field (int32 varints).
    pub fn get_packed_bool(&mut self) -> Result<IteratorRange<ConstVarintIterator<'a, i32>>, Exception> {
        self.get_packed_varint::<i32>()
    }

    /// Consume current "repeated packed enum" field.
    pub fn get_packed_enum(&mut self) -> Result<IteratorRange<ConstVarintIterator<'a, i32>>, Exception> {
        self.get_packed_varint::<i32>()
    }

    /// Consume current "repeated packed int32" field.
    pub fn get_packed_int32(&mut self) -> Result<IteratorRange<ConstVarintIterator<'a, i32>>, Exception> {
        self.get_packed_varint::<i32>()
    }

    /// Consume current "repeated packed sint32" field.
    pub fn get_packed_sint32(&mut self) -> Result<IteratorRange<ConstSvarintIterator<'a, i32>>, Exception> {
        self.get_packed_svarint::<i32>()
    }

    /// Consume current "repeated packed uint32" field.
    pub fn get_packed_uint32(&mut self) -> Result<IteratorRange<ConstVarintIterator<'a, u32>>, Exception> {
        self.get_packed_varint::<u32>()
    }

    /// Consume current "repeated packed int64" field.
    pub fn get_packed_int64(&mut self) -> Result<IteratorRange<ConstVarintIterator<'a, i64>>, Exception> {
        self.get_packed_varint::<i64>()
    }

    /// Consume current "repeated packed sint64" field.
    pub fn get_packed_sint64(&mut self) -> Result<IteratorRange<ConstSvarintIterator<'a, i64>>, Exception> {
        self.get_packed_svarint::<i64>()
    }

    /// Consume current "repeated packed uint64" field.
    pub fn get_packed_uint64(&mut self) -> Result<IteratorRange<ConstVarintIterator<'a, u64>>, Exception> {
        self.get_packed_varint::<u64>()
    }

    /// Consume current "repeated packed fixed32" field.
    pub fn get_packed_fixed32(&mut self) -> Result<IteratorRange<ConstFixedIterator<'a, u32>>, Exception> {
        self.packed_fixed::<u32>()
    }

    /// Consume current "repeated packed sfixed32" field.
    pub fn get_packed_sfixed32(&mut self) -> Result<IteratorRange<ConstFixedIterator<'a, i32>>, Exception> {
        self.packed_fixed::<i32>()
    }

    /// Consume current "repeated packed fixed64" field.
    pub fn get_packed_fixed64(&mut self) -> Result<IteratorRange<ConstFixedIterator<'a, u64>>, Exception> {
        self.packed_fixed::<u64>()
    }

    /// Consume current "repeated packed sfixed64" field.
    pub fn get_packed_sfixed64(&mut self) -> Result<IteratorRange<ConstFixedIterator<'a, i64>>, Exception> {
        self.packed_fixed::<i64>()
    }

    /// Consume current "repeated packed float" field.
    pub fn get_packed_float(&mut self) -> Result<IteratorRange<ConstFixedIterator<'a, f32>>, Exception> {
        self.packed_fixed::<f32>()
    }

    /// Consume current "repeated packed double" field.
    pub fn get_packed_double(&mut self) -> Result<IteratorRange<ConstFixedIterator<'a, f64>>, Exception> {
        self.packed_fixed::<f64>()
    }
}

/// `swap(pbf_reader&, pbf_reader&)`.
pub fn swap<'a>(lhs: &mut PbfReader<'a>, rhs: &mut PbfReader<'a>) {
    lhs.swap(rhs);
}
