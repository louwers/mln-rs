use std::rc::Rc;
use std::sync::Arc;

use mln_renderer::buckets::raster_bucket::RasterBucket;
use mln_renderer::tile::raster_tile::RasterTile;
use mln_renderer::tile::tile::Tile;
use mln_renderer::tile::tile_loader::{LoaderTile, VectorTileEncoding};
use mln_style::layer::Layer;
use mln_style::layers::RasterLayer;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::{ErrorObserver, RasterTileTest, png, serializing_transport};

fn tile(test: &RasterTileTest) -> RasterTile {
    RasterTile::new(
        OverscaledTileID::from_zxy(0, 0, 0),
        "testSource".to_owned(),
        &test.tile_parameters(),
        &test.tileset,
        None,
    )
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
    tile.inner().on_error(Error::native("test"), 0);
    assert!(!tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: RasterTile.onParsed
#[test]
fn on_parsed() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    tile.inner().on_parsed(Some(Arc::new(RasterBucket::new(PremultipliedImage::new(Size::new(0, 0))))), 0);
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

// gtest: RasterTile.onParsedEmpty. Unlike in C++, a parse without data is a tile with nothing in
// it, which is renderable and draws nothing, as for an empty response (HTTP 204) in GL JS.
#[test]
fn on_parsed_empty() {
    let test = RasterTileTest::new();
    let tile = tile(&test);
    tile.inner().on_parsed(None, 0);
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
    let layer = RasterLayer::new("r", "s").base_impl();
    let wait = || {
        while !tile.is_complete() {
            test.run_loop.run_once();
        }
    };

    tile.inner().set_data(Some(png(16, 16)), VectorTileEncoding::Mvt);
    wait();
    assert!(tile.is_renderable());
    assert!(tile.create_render_data().get_bucket(&*layer).is_some());

    LoaderTile::set_missing(&**tile.inner(), VectorTileEncoding::Mvt);
    wait();
    assert!(!tile.is_renderable());
    assert!(tile.create_render_data().get_bucket(&*layer).is_none());

    tile.inner().set_data(None, VectorTileEncoding::Mvt);
    wait();
    assert!(tile.is_renderable());
    assert!(tile.create_render_data().get_bucket(&*layer).is_none());
}

// Port-only: raster tiles whose workers are in worker instances on other
// threads get their images back through the codec; an image that doesn't
// decode becomes the tile's error.
#[test]
fn parse_through_the_codec() {
    let test = RasterTileTest::new();
    let mut parameters = test.tile_parameters();
    parameters.tile_worker_transport = serializing_transport(2);
    let errors = Rc::new(ErrorObserver::default());
    // Two tiles, so both instances get one.
    let tiles: Vec<RasterTile> = (0..2)
        .map(|x| {
            let id = OverscaledTileID::from_zxy(1, x, 0);
            RasterTile::new(id, "s".to_owned(), &parameters, &test.tileset, Some(errors.clone()))
        })
        .collect();
    for (tile, size) in tiles.iter().zip([16, 32]) {
        tile.inner().set_data(Some(png(size, size)), VectorTileEncoding::Mvt);
        assert!(!tile.is_complete());
    }
    while !tiles.iter().all(mln_renderer::tile::Tile::is_complete) {
        test.run_loop.run_once();
    }
    let layer = RasterLayer::new("r", "s").base_impl();
    for (tile, size) in tiles.iter().zip([16, 32]) {
        assert!(tile.is_renderable());
        let render_data = tile.create_render_data();
        let bucket = render_data.get_bucket(&*layer).expect("a bucket");
        let image = bucket.downcast_ref::<RasterBucket>().unwrap().image.clone().unwrap();
        assert_eq!(image.size, Size::new(size, size));
    }
    assert!(errors.0.borrow().is_empty());

    tiles[0].inner().set_data(Some(Arc::new(b"not an image".to_vec())), VectorTileEncoding::Mvt);
    while !tiles[0].is_complete() {
        test.run_loop.run_once();
    }
    assert_eq!(errors.0.borrow().len(), 1);
    // It keeps the image it had.
    assert!(tiles[0].is_renderable());
}
