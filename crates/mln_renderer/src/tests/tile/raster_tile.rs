use std::rc::Rc;
use std::sync::Arc;

use mln_style::layer::Layer;
use mln_style::layers::RasterLayer;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::{ErrorObserver, RasterTileTest, png};
use crate::buckets::raster_bucket::RasterBucket;
use crate::tile::job::JobResult;
use crate::tile::raster_tile::{RasterKind, RasterTile};
use crate::tile::tile::Tile;
use crate::tile::tile_loader::{LoaderTile, VectorTileEncoding};

fn tile(test: &RasterTileTest) -> RasterTile {
    test.tile(OverscaledTileID::from_zxy(0, 0, 0), RasterKind::Image, None)
}

// gtest: RasterTile.setError
#[test]
fn set_error() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    LoaderTile::set_error(&**tile.inner(), Error::native("test"));
    assert!(!tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: RasterTile.onError
#[test]
fn on_error() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    tile.inner().on_error(Error::native("test"));
    assert!(!tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: RasterTile.onParsed
#[test]
fn on_parsed() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    tile.inner().apply(JobResult::Raster(PremultipliedImage::new(Size::new(0, 0))));
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

// gtest: RasterTile.onParsedEmpty. Unlike in C++, a parse without data is a tile with nothing in
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
    let layer = RasterLayer::new("r", "s").base_impl();

    LoaderTile::set_data(&**tile.inner(), Some(png(16, 16)), VectorTileEncoding::Mvt);
    test.settle(&mut driver, &[&tile]);
    assert!(tile.is_renderable());
    assert!(tile.create_render_data().get_bucket(&*layer).is_some());

    LoaderTile::set_missing(&**tile.inner(), VectorTileEncoding::Mvt);
    test.settle(&mut driver, &[&tile]);
    assert!(!tile.is_renderable());
    assert!(tile.create_render_data().get_bucket(&*layer).is_none());

    LoaderTile::set_data(&**tile.inner(), None, VectorTileEncoding::Mvt);
    test.settle(&mut driver, &[&tile]);
    assert!(tile.is_renderable());
    assert!(tile.create_render_data().get_bucket(&*layer).is_none());
}

// Port-only: raster tiles whose jobs run in worker instances on other threads get their images
// back through the codec; an image that doesn't decode becomes the tile's error.
#[test]
fn decode_through_the_codec() {
    let test = RasterTileTest::new();
    let mut driver = test.remote_driver(2);
    let errors = Rc::new(ErrorObserver::default());
    // Two tiles, so both instances get one.
    let tiles: Vec<RasterTile> = (0..2)
        .map(|x| {
            let observer = Rc::<ErrorObserver>::clone(&errors);
            test.tile(OverscaledTileID::from_zxy(1, x, 0), RasterKind::Image, Some(observer))
        })
        .collect();
    let all: Vec<&dyn Tile> = tiles.iter().map(|tile| -> &dyn Tile { tile }).collect();
    for (tile, size) in tiles.iter().zip([16, 32]) {
        LoaderTile::set_data(&**tile.inner(), Some(png(size, size)), VectorTileEncoding::Mvt);
        assert!(!tile.is_complete());
    }
    test.settle(&mut driver, &all);
    let layer = RasterLayer::new("r", "s").base_impl();
    for (tile, size) in tiles.iter().zip([16, 32]) {
        assert!(tile.is_renderable());
        let render_data = tile.create_render_data();
        let bucket = render_data.get_bucket(&*layer).expect("a bucket");
        let image = bucket.downcast_ref::<RasterBucket>().unwrap().image.clone().unwrap();
        assert_eq!(image.size, Size::new(size, size));
    }
    assert!(errors.0.borrow().is_empty());

    LoaderTile::set_data(&**tiles[0].inner(), Some(Arc::new(b"not an image".to_vec())), VectorTileEncoding::Mvt);
    test.settle(&mut driver, &all[..1]);
    assert_eq!(errors.0.borrow().len(), 1);
    // It keeps the image it had.
    assert!(tiles[0].is_renderable());
}
