use std::rc::Rc;

use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::tileset::RasterEncoding;

use super::{ErrorObserver, RasterTileTest, png};
use crate::geometry::dem_data::DEMData;
use crate::tile::job::JobResult;
use crate::tile::raster_tile::{DEMTileNeighbors, RasterKind, RasterTile};
use crate::tile::tile::Tile;
use crate::tile::tile_loader::{LoaderTile, VectorTileEncoding};

const DEM: RasterKind = RasterKind::Dem(RasterEncoding::Mapbox);

fn tile(test: &RasterTileTest) -> RasterTile {
    test.tile(OverscaledTileID::from_zxy(0, 0, 0), DEM, None)
}

// gtest: RasterDEMTile.setError
#[test]
fn set_error() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    LoaderTile::set_error(&**tile.inner(), Error::native("test"));
    assert!(!tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: RasterDEMTile.onError
#[test]
fn on_error() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    tile.inner().on_error(Error::native("test"));
    assert!(!tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: RasterDEMTile.onParsed
#[test]
fn on_parsed() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    let dem = DEMData::new(&PremultipliedImage::new(Size::new(16, 16)), RasterEncoding::Mapbox).expect("DEMData");
    tile.inner().apply(JobResult::Dem(dem));
    assert!(tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());

    // Make sure that once we've had a renderable tile and then receive
    // erroneous data, we retain the previously rendered data and keep the tile
    // renderable.
    LoaderTile::set_error(&**tile.inner(), Error::native("Connection offline"));
    assert!(tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());

    // Then simulate a parsing failure and make sure that we keep it renderable
    // in this situation as well.
    tile.inner().on_error(Error::native("Parse error"));
    assert!(tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: RasterDEMTile.onParsedEmpty. Unlike in C++, a parse without data is a tile with nothing in
// it, which is renderable and draws nothing, as for an empty response (HTTP 204) in GL JS.
#[test]
fn on_parsed_empty() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    LoaderTile::set_data(&**tile.inner(), None, VectorTileEncoding::Mvt);
    assert!(tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// Port-only: a tile the tileset doesn't have (HTTP 404) isn't renderable, so the pyramid draws a
// tile from another zoom level in its place. An empty tile is, and draws nothing. Each replaces
// the image the tile had.
#[test]
fn missing_and_empty() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    let mut driver = test.pool_driver();

    LoaderTile::set_data(&**tile.inner(), Some(png(16, 16)), VectorTileEncoding::Mvt);
    test.settle(&mut driver, &[&tile]);
    assert!(tile.is_renderable());
    assert!(tile.get_bucket().is_some());

    LoaderTile::set_missing(&**tile.inner(), VectorTileEncoding::Mvt);
    test.settle(&mut driver, &[&tile]);
    assert!(!tile.is_renderable());
    assert!(tile.get_bucket().is_none());

    LoaderTile::set_data(&**tile.inner(), None, VectorTileEncoding::Mvt);
    test.settle(&mut driver, &[&tile]);
    assert!(tile.is_renderable());
    assert!(tile.get_bucket().is_none());
}

// Port-only: raster-dem tiles whose jobs run in worker instances on other threads get their DEMs
// back through the codec, borders included, and backfill them from each other.
#[test]
fn decode_and_backfill_through_the_codec() {
    let test = RasterTileTest::new();
    let mut driver = test.remote_driver(2);
    let errors = Rc::new(ErrorObserver::default());
    // Neighbours, so both instances get one.
    let tiles: Vec<RasterTile> = (0..2)
        .map(|x| test.tile(OverscaledTileID::from_zxy(1, x, 0), DEM, Some(Rc::<ErrorObserver>::clone(&errors))))
        .collect();
    let all: Vec<&dyn Tile> = tiles.iter().map(|tile| -> &dyn Tile { tile }).collect();
    for tile in &tiles {
        LoaderTile::set_data(&**tile.inner(), Some(png(16, 16)), VectorTileEncoding::Mvt);
    }
    test.settle(&mut driver, &all);
    for tile in &tiles {
        assert!(tile.is_renderable());
        let bucket = tile.get_bucket().expect("a bucket");
        let dem = bucket.get_dem_data();
        assert_eq!((dem.dim, dem.stride, dem.encoding), (16, 18, RasterEncoding::Mapbox));
    }

    tiles[0].backfill_border(&tiles[1], DEMTileNeighbors::RIGHT);
    assert_eq!(tiles[0].neighboring_tiles() & DEMTileNeighbors::RIGHT, DEMTileNeighbors::RIGHT);
    let (left, right) = (tiles[0].get_bucket().unwrap(), tiles[1].get_bucket().unwrap());
    for y in 0..16 {
        assert_eq!(left.get_dem_data().get(16, y), right.get_dem_data().get(0, y));
    }

    // The DEMData constructor's error crosses as the tile's.
    LoaderTile::set_data(&**tiles[1].inner(), Some(png(16, 8)), VectorTileEncoding::Mvt);
    test.settle(&mut driver, &all[1..]);
    assert_eq!(*errors.0.borrow(), ["raster-dem tiles must be square."]);
    assert!(tiles[1].is_renderable());
}
