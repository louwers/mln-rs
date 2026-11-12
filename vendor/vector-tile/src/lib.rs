//! Ports `vendor/vector-tile` (`mapbox::vector_tile` 1.0.5), the Mapbox
//! Vector Tile reader the C++ core decodes MVT tiles with:
//!
//! - `vector_tile/vector_tile_config.hpp` → [`config`] (the protobuf tag
//!   constants, `GeomType`, `CommandType`),
//! - `vector_tile/version.hpp` → the `VECTOR_TILE_VERSION_*` constants,
//! - `vector_tile.hpp` → [`Buffer`], [`Layer`], [`Feature`],
//!   [`parse_value`], the geometry command decoding
//!   ([`Feature::get_geometries`]).
//!
//! The types borrow the tile bytes (`&'a [u8]`) like the C++ `data_view`s.
//! Where the C++ throws, functions return [`Error`] with the same exception
//! kind and `what()` text.
//!
//! Strings: C++ keeps layer names, keys and string values as `std::string`
//! bytes. The port keeps names and keys as bytes internally (layer lookup,
//! ordering and key comparison work on bytes, as in C++) and converts to
//! Rust `String`s, which must be UTF-8, with `String::from_utf8_lossy` where
//! they leave the crate (`get_name`, `layer_names`, property keys and string
//! values).

pub mod config;
mod error;
mod vector_tile;

pub use config::{CommandType, GeomType};
pub use error::Error;
pub use vector_tile::{
    Buffer, Coordinate, Feature, Layer, PointType, PointsArrayType, PointsArraysType, PropertiesType, parse_value,
};

/// `VECTOR_TILE_VERSION_MAJOR`.
pub const VECTOR_TILE_VERSION_MAJOR: u32 = 1;
/// `VECTOR_TILE_VERSION_MINOR`.
pub const VECTOR_TILE_VERSION_MINOR: u32 = 0;
/// `VECTOR_TILE_VERSION_PATCH`.
pub const VECTOR_TILE_VERSION_PATCH: u32 = 5;
/// `VECTOR_TILE_VERSION_CODE`.
pub const VECTOR_TILE_VERSION_CODE: u32 =
    VECTOR_TILE_VERSION_MAJOR * 10000 + VECTOR_TILE_VERSION_MINOR * 100 + VECTOR_TILE_VERSION_PATCH;
/// `VECTOR_TILE_VERSION_STRING`.
pub const VECTOR_TILE_VERSION_STRING: &str = "1.0.5";

#[cfg(test)]
mod vector_tile_test;
