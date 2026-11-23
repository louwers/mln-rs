use std::sync::Arc;

use mln_style::layer_type::SourceKind;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::{RasterTileTest, png};
use crate::tests::OneTile;
use crate::tests::tile_jobs_test::Manual;
use crate::tile::contents::Contents;
use crate::tile::job::JobResult;
use crate::tile::raster::RasterContents;
use crate::tile::tile_workers::InlineWorkers;

fn tiles(test: &RasterTileTest) -> OneTile {
    test.tiles(SourceKind::Raster, OverscaledTileID::from_zxy(0, 0, 0), Box::new(InlineWorkers))
}

/// The size of the image tile `id` shows.
fn image_size(tiles: &OneTile, id: &OverscaledTileID) -> Option<Size> {
    match tiles.tile_at(id).contents() {
        Some(Contents::Raster(bucket)) => bucket.image().map(|image| image.size),
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

// What the layers make of a raster tile's image goes when the part of the
// tile it draws changes: its contents take a new identity, and keep it
// through a mask that changes nothing. Its texture, which the contents make
// once they upload, stays.
#[test]
fn contents_take_a_new_identity_when_their_geometry_changes_and_keep_their_texture() {
    let mut context = crate::gfx::context::Resources::detached();
    let mut contents = RasterContents::new(PremultipliedImage::new(Size::new(4, 4)));
    assert!(contents.texture().is_none(), "no texture before the upload");
    contents.upload(&mut context);
    let texture = contents.texture().cloned().expect("a texture once uploaded");
    let first = contents.id();
    contents.set_mask([CanonicalTileID::new(0, 0, 0)].into());
    assert_eq!(first, contents.id(), "the whole tile, as before");
    contents.set_mask([CanonicalTileID::new(1, 0, 0)].into());
    let masked = contents.id();
    assert_ne!(first, masked);
    contents.unmask();
    assert_ne!(masked, contents.id());
    let warped = contents.id();
    contents.set_geometry(Vec::new(), Vec::new(), Vec::new());
    assert_ne!(warped, contents.id(), "an image source's image warped again");
    contents.upload(&mut context);
    assert_eq!(Some(&texture), contents.texture());
}
