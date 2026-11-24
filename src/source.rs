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
//!
//! A small source type, each of whose tiles holds its zoom level, which its
//! origin gives the tile as it starts, has both halves, a registry that adds
//! it and a style that loads a source of it. The example checks that the
//! type registers and parses; its origin runs only on a map, as the `stamp`
//! source type of the extension tests (`crates/mln_extension_tests`) does:
//!
//! ```
//! use std::convert::Infallible;
//! use std::sync::Arc;
//!
//! use mln_rs::mln_render::RegistryBuilder;
//! use mln_rs::mln_style::Style;
//! use mln_rs::source::conversion::Error;
//! use mln_rs::source::{
//!     Convertible, Cover, DataOrigin, Functions, LoadContext, OriginUpdate, Range, Source, SourceData, SourceKind,
//!     SourceType, SourceTypeInfo, StyleSourceType, Supply, ValueKind, ValueTiles, WorldCopy,
//! };
//!
//! /// What a tile of the type holds, which layer types view as its value.
//! struct TileZoom(u8);
//!
//! /// Both halves of the source type.
//! struct ZoomSource;
//!
//! impl StyleSourceType for ZoomSource {
//!     const INFO: SourceTypeInfo = SourceTypeInfo {
//!         name: "org.example.zoom",
//!         layer_kind: SourceKind::Value(ValueKind::of::<TileZoom>()),
//!         from_json: true,
//!     };
//!     type Options = ();
//!     type Description = ();
//!
//!     fn from_json(_value: &Convertible<'_>, _functions: &Arc<Functions>, _error: &mut Error) -> Option<SourceData<Self>> {
//!         Some(SourceData::new(()))
//!     }
//! }
//!
//! impl SourceType for ZoomSource {
//!     type Data = ValueTiles<TileZoom>;
//!
//!     fn supply(_source: &Source) -> Supply<ValueTiles<TileZoom>> {
//!         Supply::tiles(ZoomOrigin)
//!     }
//! }
//!
//! /// Where the source's tiles get their values.
//! struct ZoomOrigin;
//!
//! impl DataOrigin for ZoomOrigin {
//!     type Data = ValueTiles<TileZoom>;
//!     type Tile = ();
//!     type Reply = Infallible;
//!
//!     /// The tiles that cover the view, at every zoom level.
//!     fn update(&mut self, _update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
//!         Some(Cover {
//!             tile_size: 512,
//!             zoom_range: Range::new(0, 22),
//!             bounds: None,
//!             round_zoom: false,
//!             overscale: false,
//!             prefetch: false,
//!             synchronous: false,
//!         })
//!     }
//!
//!     /// A tile in a new world copy keeps the same tile's value in
//!     /// another; any other gets its own.
//!     fn start(&mut self, cx: &mut LoadContext<'_, '_, Self>, copy: Option<WorldCopy<'_, ()>>) {
//!         if !copy.is_some_and(|copy| copy.take(cx)) {
//!             cx.deliver(Some(Arc::new(TileZoom(cx.id().canonical.z))));
//!         }
//!     }
//!
//!     fn receive(&mut self, _tile: &mut (), reply: Infallible, _cx: &mut LoadContext<'_, '_, Self>) {
//!         match reply {}
//!     }
//! }
//!
//! let mut builder = RegistryBuilder::new();
//! builder.add_source_type::<ZoomSource>()?;
//! let registry = builder.build();
//!
//! let json = r#"{"version":8,"sources":{"zooms":{"type":"org.example.zoom"}},"layers":[]}"#;
//! let style = Style::from_json(registry.style(), json, None)?;
//! assert!(style.get_source("zooms").is_some());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub use mln_map_types::MapMode;
pub use mln_render::source::*;
pub use mln_storage::{Data, FileSource, RequestOrder, Resource, TileFormat};
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
