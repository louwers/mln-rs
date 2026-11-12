// Ports vendor/protozero/include/protozero/types.hpp.

/// `pbf_tag_type`: the type used for field tags (field numbers).
pub type PbfTagType = u32;

/// `pbf_length_type`: the type used for length values, such as the length
/// of a field.
pub type PbfLengthType = u32;

/// `enum class pbf_wire_type : uint32_t`.
///
/// A newtype rather than a Rust enum because, as in C++, the reader stores
/// the three wire-type bits of a key *before* it rejects unknown values, so
/// `wire_type()` can hold 3, 4, 6 or 7 after an `UnknownPbfWireType` error.
/// The associated constants have the C++ enumerator names and work as
/// `match` patterns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PbfWireType(pub u32);

#[allow(non_upper_case_globals)]
impl PbfWireType {
    /// int32/64, uint32/64, sint32/64, bool, enum
    pub const Varint: PbfWireType = PbfWireType(0);
    /// fixed64, sfixed64, double
    pub const Fixed64: PbfWireType = PbfWireType(1);
    /// string, bytes, nested messages, packed repeated fields
    pub const LengthDelimited: PbfWireType = PbfWireType(2);
    /// fixed32, sfixed32, float
    pub const Fixed32: PbfWireType = PbfWireType(5);
    /// used for default setting in this library
    pub const Unknown: PbfWireType = PbfWireType(99);
}

impl Default for PbfWireType {
    fn default() -> Self {
        PbfWireType::Unknown
    }
}

/// Get the tag and wire type of the current field in one integer suitable
/// for comparison with a switch statement.
#[must_use]
pub const fn tag_and_type(tag: PbfTagType, wire_type: PbfWireType) -> u32 {
    (tag << 3) | wire_type.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_and_type_combines() {
        assert_eq!(tag_and_type(1, PbfWireType::LengthDelimited), 0x0a);
        assert_eq!(tag_and_type(3, PbfWireType::LengthDelimited), 0x1a);
        assert_eq!(tag_and_type(15, PbfWireType::Varint), 0x78);
        assert_eq!(PbfWireType::default(), PbfWireType::Unknown);
    }
}
