// Ports vendor/protozero/include/protozero/exception.hpp.

use std::fmt;

/// The protozero exceptions. C++ has `struct exception : std::exception`
/// and one derived struct per error; here they are the variants of one enum,
/// and [`Exception::what`] returns the C++ `what()` text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Exception {
    /// `protozero::exception`: the base, never thrown by the reader itself.
    Exception,
    /// `varint_too_long_exception`: a varint longer than the maximum that
    /// fits a 64 bit integer (corrupt data).
    VarintTooLong,
    /// `unknown_pbf_wire_type_exception`: the wire type of a field is
    /// unknown (corrupt data).
    UnknownPbfWireType,
    /// `end_of_buffer_exception`: not enough bytes left to read a field.
    EndOfBuffer,
    /// `invalid_tag_exception`: tags must be between 1 and 2^29-1, and
    /// 19000 to 19999 are not allowed.
    InvalidTag,
    /// `invalid_length_exception`: the length of a packed fixed-size field
    /// is not a multiple of the size of the type.
    InvalidLength,
}

impl Exception {
    /// Returns the explanatory string (`what()`).
    #[must_use]
    pub const fn what(&self) -> &'static str {
        match self {
            Exception::Exception => "pbf exception",
            Exception::VarintTooLong => "varint too long exception",
            Exception::UnknownPbfWireType => "unknown pbf field type exception",
            Exception::EndOfBuffer => "end of buffer exception",
            Exception::InvalidTag => "invalid tag exception",
            Exception::InvalidLength => "invalid length exception",
        }
    }
}

impl fmt::Display for Exception {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.what())
    }
}

impl std::error::Error for Exception {}

#[cfg(test)]
mod tests {
    // Ports vendor/protozero/test/unit/test_exceptions.cpp.
    use super::Exception;

    #[test]
    fn exceptions_messages_for_pbf_exception() {
        assert_eq!(Exception::Exception.what(), "pbf exception");
    }

    #[test]
    fn exceptions_messages_for_varint_too_long() {
        assert_eq!(Exception::VarintTooLong.what(), "varint too long exception");
    }

    #[test]
    fn exceptions_messages_for_unknown_pbf_field_type() {
        assert_eq!(Exception::UnknownPbfWireType.what(), "unknown pbf field type exception");
    }

    #[test]
    fn exceptions_messages_for_end_of_buffer() {
        assert_eq!(Exception::EndOfBuffer.what(), "end of buffer exception");
    }

    #[test]
    fn exceptions_messages_for_invalid_tag() {
        assert_eq!(Exception::InvalidTag.what(), "invalid tag exception");
    }

    #[test]
    fn exceptions_messages_for_invalid_length() {
        assert_eq!(Exception::InvalidLength.what(), "invalid length exception");
        assert_eq!(Exception::InvalidLength.to_string(), "invalid length exception");
    }
}
