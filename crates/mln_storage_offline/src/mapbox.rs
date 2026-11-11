// The canonicalize* half of src/mln/util/mapbox.cpp, typed by
// `style::SourceType` as in C++; mln_util::mapbox implements it.

use mln_style_types::SourceType;
pub use mln_util::mapbox::{canonicalize_glyph_url, canonicalize_source_url, canonicalize_sprite_url};
use mln_util::tile_server_options::TileServerOptions;
use mln_util::tileset::Tileset;

fn is_raster(type_: SourceType) -> bool {
    type_ == SourceType::Raster || type_ == SourceType::RasterDEM
}

/// `canonicalizeTileURL(tileServerOptions, str, type, tileSize)`.
pub fn canonicalize_tile_url(
    tile_server_options: &TileServerOptions,
    s: &str,
    type_: SourceType,
    tile_size: u16,
) -> String {
    mln_util::mapbox::canonicalize_tile_url(tile_server_options, s, is_raster(type_), tile_size)
}

/// `canonicalizeTileset(tileServerOptions, tileset, sourceURL, type, tileSize)`.
pub fn canonicalize_tileset(
    tile_server_options: &TileServerOptions,
    tileset: &mut Tileset,
    source_url: &str,
    type_: SourceType,
    tile_size: u16,
) {
    mln_util::mapbox::canonicalize_tileset(tile_server_options, tileset, source_url, is_raster(type_), tile_size);
}
