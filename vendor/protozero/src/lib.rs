//! Ports the reader half of `vendor/protozero` (version 1.7.1), the
//! minimalistic protocol buffer decoder the C++ core uses for vector tiles
//! (`vendor/vector-tile`) and glyph PBFs (`text/glyph_pbf.cpp`):
//!
//! - `exception.hpp` → [`exception`] ([`Exception`], one enum for the
//!   exception structs, `what()` texts unchanged),
//! - `varint.hpp` → [`varint`] (decode/skip with the folly fast path and the
//!   strong exception guarantee, zigzag, `add_varint_to_buffer`),
//! - `types.hpp` → [`types`] ([`PbfWireType`], [`tag_and_type`]),
//! - `data_view.hpp` → [`data_view`] ([`DataView`] = `&[u8]`),
//! - `iterators.hpp` → [`iterators`] (`const_fixed_iterator`,
//!   `const_varint_iterator`, `const_svarint_iterator`, `iterator_range`),
//! - `pbf_reader.hpp` → [`pbf_reader`] ([`PbfReader`]),
//! - `version.hpp` → the `PROTOZERO_VERSION_*` constants below.
//!
//! Every reading function returns `Result<_, Exception>` where the C++
//! throws. Not ported, because nothing in the C++ core uses them: the writer
//! side (`basic_pbf_writer`, `pbf_builder`, `buffer_*`), `pbf_message`, and
//! `byteswap.hpp` (fixed-size values are always decoded little-endian, which
//! is what protozero's byte swapping achieves on big-endian machines).
//!
//! `protozero_assert`: the "call next() before accessing field value"
//! preconditions are `debug_assert!`s, and in debug builds `skip_bytes`
//! resets the tag to 0 exactly as the C++ does without `NDEBUG`. The
//! wire-type preconditions ("not a varint", ...) are not checked: they depend
//! on the input data, and the port does what the C++ does with `NDEBUG`.

pub mod data_view;
pub mod exception;
pub mod iterators;
pub mod pbf_reader;
pub mod types;
pub mod varint;

pub use data_view::DataView;
pub use exception::Exception;
pub use iterators::{
    ConstFixedIterator, ConstSvarintIterator, ConstVarintIterator, FixedValue, IteratorRange, SvarintValue, VarintValue,
};
pub use pbf_reader::PbfReader;
pub use types::{PbfLengthType, PbfTagType, PbfWireType, tag_and_type};
pub use varint::{
    MAX_VARINT_LENGTH, add_varint_to_buffer, decode_varint, decode_zigzag32, decode_zigzag64, encode_zigzag32,
    encode_zigzag64, length_of_varint, skip_varint,
};

/// `PROTOZERO_VERSION_MAJOR`.
pub const PROTOZERO_VERSION_MAJOR: u32 = 1;
/// `PROTOZERO_VERSION_MINOR`.
pub const PROTOZERO_VERSION_MINOR: u32 = 7;
/// `PROTOZERO_VERSION_PATCH`.
pub const PROTOZERO_VERSION_PATCH: u32 = 1;
/// `PROTOZERO_VERSION_CODE`.
pub const PROTOZERO_VERSION_CODE: u32 =
    PROTOZERO_VERSION_MAJOR * 10000 + PROTOZERO_VERSION_MINOR * 100 + PROTOZERO_VERSION_PATCH;
/// `PROTOZERO_VERSION_STRING`.
pub const PROTOZERO_VERSION_STRING: &str = "1.7.1";

#[cfg(test)]
mod basic_test;
#[cfg(test)]
mod test_util;
#[cfg(test)]
mod varint_test;
