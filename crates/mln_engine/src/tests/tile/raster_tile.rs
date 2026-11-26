use std::sync::Arc;

use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::layer::SourceKind;
use mln_style::size::Size;

use super::{RasterTileTest, png};
use crate::tests::OneTile;
use crate::tests::tile_jobs_test::Manual;
use crate::tile::OverscaledTileID;
use crate::tile_store::contents::Contents;
use crate::worker::job::JobResult;
use crate::worker::tile_workers::InlineWorkers;

fn tiles(test: &RasterTileTest) -> OneTile {
    test.tiles(SourceKind::Raster, OverscaledTileID::from_zxy(0, 0, 0), Box::new(InlineWorkers))
}

/// The size of the image tile `id` shows.
fn image_size(tiles: &OneTile, id: &OverscaledTileID) -> Option<Size> {
    match tiles.tile_at(id).contents() {
        Some(Contents::Raster(raster)) => Some(raster.image.size),
        _ => None,
    }
}

// gtest: RasterTile.setError
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

// gtest: RasterTile.onError
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

// gtest: RasterTile.onParsed
#[test]
fn on_parsed() {
    let test = RasterTileTest::new();
    let mut tiles = tiles(&test);
    tiles.with(|tile| tile.apply_decode_now(JobResult::Raster(PremultipliedImage::new(Size::new(0, 0)))));
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

// gtest: RasterTile.onParsedEmpty. Unlike in C++, a parse without data is a tile with nothing in
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
    let mut tiles = test.pool_tiles(SourceKind::Raster);
    let id = tiles.id;

    tiles.set_image(&id, Some(png(16, 16)));
    tiles.settle(&[id]);
    assert!(tiles.tile().is_renderable());
    assert!(image_size(&tiles, &id).is_some());

    tiles.set_missing(&id);
    tiles.settle(&[id]);
    assert!(!tiles.tile().is_renderable());
    assert!(tiles.tile().contents().is_none());

    tiles.set_image(&id, None);
    tiles.settle(&[id]);
    assert!(tiles.tile().is_renderable());
    assert!(matches!(tiles.tile().contents(), Some(Contents::Empty)));
}

// Port-only: raster tiles whose jobs run in worker instances on other threads get their images
// back through the codec; an image that doesn't decode becomes the tile's error.
#[test]
fn decode_through_the_codec() {
    let test = RasterTileTest::new();
    // Two tiles, so both instances get one.
    let mut tiles = test.remote_tiles(SourceKind::Raster, 2);
    let ids = [OverscaledTileID::from_zxy(1, 0, 0), OverscaledTileID::from_zxy(1, 1, 0)];
    for (id, size) in ids.iter().zip([16, 32]) {
        tiles.set_image(id, Some(png(size, size)));
        assert!(!tiles.tile_at(id).is_complete());
    }
    tiles.settle(&ids);
    for (id, size) in ids.iter().zip([16, 32]) {
        assert!(tiles.tile_at(id).is_renderable());
        assert_eq!(image_size(&tiles, id), Some(Size::new(size, size)));
    }
    assert_eq!(tiles.errors(), Vec::<String>::new());

    tiles.set_image(&ids[0], Some(Arc::new(b"not an image".to_vec())));
    tiles.settle(&ids[..1]);
    assert_eq!(tiles.errors().len(), 1);
    // It keeps the image it had.
    assert!(tiles.tile_at(&ids[0]).is_renderable());
}

// Port-only: a load without an image while the tile decodes shows nothing at once, and the decode
// in flight stays the tile's one job: an image that comes before it replies decodes after.
#[test]
fn an_empty_load_during_a_decode_keeps_one_decode_in_flight() {
    let test = RasterTileTest::new();
    let manual = Manual::default();
    let mut tiles = test.tiles(SourceKind::Raster, OverscaledTileID::from_zxy(0, 0, 0), Box::new(manual.clone()));
    let id = tiles.id;
    tiles.set_image(&id, Some(png(16, 16)));
    tiles.update();
    assert_eq!(manual.submitted(), 1);
    let decode = manual.take();

    tiles.set_image(&id, None);
    assert!(matches!(tiles.tile().contents(), Some(Contents::Empty)));
    assert!(tiles.tile().is_complete());
    tiles.set_image(&id, Some(png(8, 8)));
    tiles.update();
    assert_eq!(manual.submitted(), 1, "one decode in flight");

    manual.reply(InlineWorkers::run(decode));
    tiles.update();
    assert!(matches!(tiles.tile().contents(), Some(Contents::Empty)), "the stale decode isn't shown");
    assert_eq!(manual.submitted(), 2);
    manual.run();
    tiles.update();
    assert_eq!(image_size(&tiles, &id), Some(Size::new(8, 8)));
    assert!(tiles.tile().is_complete());
}
