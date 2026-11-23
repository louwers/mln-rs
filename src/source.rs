//! What a source type's author writes with, from its home crates: the
//! source type contract ([`mln_render::source`], whose doc states it), the
//! style half's names (`mln_style`), and the values their signatures name,
//! with [`ValueKind`] for a type whose tiles hold a value of its own, which
//! layer types view through [`mln_render::layer::ValueTiles`]. A source type
//! registers with
//! [`RegistryBuilder::add_source_type`](mln_render::RegistryBuilder::add_source_type).
//! With the `runtime` feature, its own commands go through a map's generic
//! source paths: `Map::add_style_source`, `Map::apply_style_edit`,
//! `Map::command_style_source`, `Map::query_style_source` and
//! `Map::set_style_source_data`.

pub use mln_map_types::MapMode;
pub use mln_render::source::*;
pub use mln_storage::{Data, FileSource, RequestOrder, Resource};
pub use mln_style::Edit;
pub use mln_style::conversion::{self, Convertible};
pub use mln_style::expression::Functions;
pub use mln_style::layer_type::{SourceKind, ValueKind};
pub use mln_style::source_type::{
    Described, Load, OfflineResources, Source, SourceCommon, SourceData, SourceTypeInfo, StyleSourceType,
};
pub use mln_tile::TileData;
pub use mln_tile_id::{CanonicalTileID, OverscaledTileID};
pub use mln_util::chrono::Timestamp;
pub use mln_util::error::Error;
pub use mln_util::geo::{LatLng, LatLngBounds};
pub use mln_util::image::PremultipliedImage;
pub use mln_util::pool::{Pool, Respond};
pub use mln_util::range::Range;
pub use mln_util::tileset::{RasterEncoding, Tileset};
