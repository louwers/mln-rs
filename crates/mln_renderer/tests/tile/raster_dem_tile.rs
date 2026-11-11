// Ports test/tile/raster_dem_tile.test.cpp.

use std::rc::Rc;
use std::sync::Arc;

use mln_renderer::buckets::hillshade_bucket::HillshadeBucket;
use mln_renderer::tile::raster_dem_tile::{DEMTileNeighbors, RasterDEMTile};
use mln_renderer::tile::tile::Tile;
use mln_renderer::tile::tile_loader::{LoaderTile, VectorTileEncoding};
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::tileset::RasterEncoding;

use super::{ErrorObserver, RasterTileTest, png, serializing_transport};

fn tile(test: &RasterTileTest) -> RasterDEMTile {
    RasterDEMTile::new(
        OverscaledTileID::from_zxy(0, 0, 0),
        "testSource".to_owned(),
        &test.tile_parameters(),
        &test.tileset,
        None,
    )
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
    tile.inner().on_error(Error::native("test"), 0);
    assert!(!tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: RasterDEMTile.onParsed
#[test]
fn on_parsed() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    let bucket = HillshadeBucket::new(PremultipliedImage::new(Size::new(16, 16)), RasterEncoding::Mapbox)
        .expect("HillshadeBucket");
    tile.inner().on_parsed(Some(Arc::new(bucket)), 0);
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
    tile.inner().on_error(Error::native("Parse error"), 0);
    assert!(tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: RasterDEMTile.onParsedEmpty
#[test]
fn on_parsed_empty() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    tile.inner().on_parsed(None, 0);
    assert!(!tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// Port-only: raster-dem tiles whose workers are in worker instances on other
// threads get their DEMs back through the codec, borders included, and
// backfill them from each other.
#[test]
fn parse_and_backfill_through_the_codec() {
    let test = RasterTileTest::new();
    let mut parameters = test.tile_parameters();
    parameters.tile_worker_transport = serializing_transport(2);
    let errors = Rc::new(ErrorObserver::default());
    // Neighbours, so both instances get one.
    let tiles: Vec<RasterDEMTile> = (0..2)
        .map(|x| {
            let id = OverscaledTileID::from_zxy(1, x, 0);
            RasterDEMTile::new(id, "s".to_owned(), &parameters, &test.tileset, Some(errors.clone()))
        })
        .collect();
    for tile in &tiles {
        tile.inner().set_data(Some(png(16, 16)), VectorTileEncoding::Mvt);
    }
    while !tiles.iter().all(|tile| tile.is_complete()) {
        test.run_loop.run_once();
    }
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
    tiles[1].inner().set_data(Some(png(16, 8)), VectorTileEncoding::Mvt);
    while !tiles[1].is_complete() {
        test.run_loop.run_once();
    }
    assert_eq!(*errors.0.borrow(), ["raster-dem tiles must be square."]);
    assert!(tiles[1].is_renderable());
}
