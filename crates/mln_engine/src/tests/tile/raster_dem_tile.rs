use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::size::Size;
use mln_style::tileset::RasterEncoding;
use mln_tile_id::OverscaledTileID;

use super::{RasterTileTest, png};
use crate::geometry::dem_data::DEMData;
use crate::tests::OneTile;
use crate::tile_store::contents::Contents;
use crate::tile_store::tile::TileKind;
use crate::worker::job::JobResult;
use crate::worker::tile_workers::InlineWorkers;

const DEM: TileKind = TileKind::Dem(RasterEncoding::Mapbox);

fn tiles(test: &RasterTileTest) -> OneTile {
    test.tiles(DEM, OverscaledTileID::from_zxy(0, 0, 0), Box::new(InlineWorkers))
}

/// The DEM tile `id` shows.
fn dem<'a>(tiles: &'a OneTile, id: &OverscaledTileID) -> Option<&'a DEMData> {
    match tiles.tile_at(id).contents() {
        Some(Contents::Dem(dem)) => Some(dem.dem()),
        _ => None,
    }
}

// gtest: RasterDEMTile.setError
#[test]
fn set_error() {
    let test = RasterTileTest::new();
    let mut tiles = tiles(&test);
    tiles.with(|tile| tile.fail(Error::native("test")));
    let tile = tiles.tile();
    assert!(!tile.is_renderable());
    assert!(tile.is_settled());
    assert!(tile.is_complete());
}

// gtest: RasterDEMTile.onError
#[test]
fn on_error() {
    let test = RasterTileTest::new();
    let mut tiles = tiles(&test);
    tiles.with(|tile| tile.fail(Error::native("test")));
    let tile = tiles.tile();
    assert!(!tile.is_renderable());
    assert!(tile.is_settled());
    assert!(tile.is_complete());
}

// gtest: RasterDEMTile.onParsed
#[test]
fn on_parsed() {
    let test = RasterTileTest::new();
    let mut tiles = tiles(&test);
    let dem = DEMData::new(&PremultipliedImage::new(Size::new(16, 16)), RasterEncoding::Mapbox).expect("DEMData");
    tiles.with(|tile| tile.apply_decode_now(JobResult::Dem(dem)));
    assert!(tiles.tile().is_renderable());
    assert!(tiles.tile().is_settled());
    assert!(tiles.tile().is_complete());

    // Make sure that once we've had a renderable tile and then receive
    // erroneous data, we retain the previously rendered data and keep the tile
    // renderable.
    tiles.with(|tile| tile.fail(Error::native("Connection offline")));
    assert!(tiles.tile().is_renderable());
    assert!(tiles.tile().is_settled());
    assert!(tiles.tile().is_complete());

    // Then simulate a parsing failure and make sure that we keep it renderable
    // in this situation as well.
    tiles.with(|tile| tile.fail(Error::native("Parse error")));
    assert!(tiles.tile().is_renderable());
    assert!(tiles.tile().is_settled());
    assert!(tiles.tile().is_complete());
}

// gtest: RasterDEMTile.onParsedEmpty. Unlike in C++, a parse without data is a tile with nothing in
// it, which is renderable and draws nothing, as for an empty response (HTTP 204) in GL JS.
#[test]
fn on_parsed_empty() {
    let test = RasterTileTest::new();
    let mut tiles = tiles(&test);
    let id = tiles.id;
    tiles.set_image(&id, None);
    assert!(tiles.tile().is_renderable());
    assert!(tiles.tile().is_settled());
    assert!(tiles.tile().is_complete());
}

// Port-only: a tile the tileset doesn't have (HTTP 404) isn't renderable, so the pyramid draws a
// tile from another zoom level in its place. An empty tile is, and draws nothing. Each replaces
// the image the tile had.
#[test]
fn missing_and_empty() {
    let test = RasterTileTest::new();
    let mut tiles = test.pool_tiles(DEM);
    let id = tiles.id;

    tiles.set_image(&id, Some(png(16, 16)));
    tiles.settle(&[id]);
    assert!(tiles.tile().is_renderable());
    assert!(dem(&tiles, &id).is_some());

    tiles.set_missing(&id);
    tiles.settle(&[id]);
    assert!(!tiles.tile().is_renderable());
    assert!(dem(&tiles, &id).is_none());

    tiles.set_image(&id, None);
    tiles.settle(&[id]);
    assert!(tiles.tile().is_renderable());
    assert!(dem(&tiles, &id).is_none());
}

// Port-only: raster-dem tiles whose jobs run in worker instances on other threads get their DEMs
// back through the codec, borders included, and the store backfills neighbours from each other as
// their DEMs arrive.
#[test]
fn decode_and_backfill_through_the_codec() {
    let test = RasterTileTest::new();
    // Neighbours, so both instances get one.
    let mut tiles = test.remote_tiles(DEM, 2);
    let ids = [OverscaledTileID::from_zxy(1, 0, 0), OverscaledTileID::from_zxy(1, 1, 0)];
    for id in &ids {
        tiles.set_image(id, Some(png(16, 16)));
    }
    tiles.settle(&ids);
    for id in &ids {
        assert!(tiles.tile_at(id).is_renderable());
        let dem = dem(&tiles, id).expect("a DEM");
        assert_eq!((dem.dim, dem.stride, dem.encoding), (16, 18, RasterEncoding::Mapbox));
    }

    let (left, right) = (dem(&tiles, &ids[0]).unwrap(), dem(&tiles, &ids[1]).unwrap());
    for y in 0..16 {
        assert_eq!(left.get(16, y), right.get(0, y));
        assert_eq!(right.get(-1, y), left.get(15, y));
    }

    // The DEMData constructor's error crosses as the tile's.
    tiles.set_image(&ids[1], Some(png(16, 8)));
    tiles.settle(&ids[1..]);
    assert_eq!(tiles.errors(), ["raster-dem tiles must be square."]);
    assert!(tiles.tile_at(&ids[1]).is_renderable());
}
