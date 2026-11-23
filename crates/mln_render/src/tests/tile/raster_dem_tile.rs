use mln_style::layer_type::SourceKind;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::tileset::RasterEncoding;

use super::{RasterTileTest, png};
use crate::geometry::dem_data::DemData;
use crate::tests::OneTile;
use crate::tile::contents::Contents;
use crate::tile::dem::DemContents;
use crate::tile::job::JobResult;
use crate::tile::tile_workers::InlineWorkers;

const DEM: SourceKind = SourceKind::RasterDem;

fn tiles(test: &RasterTileTest) -> OneTile {
    test.tiles(DEM, OverscaledTileID::from_zxy(0, 0, 0), Box::new(InlineWorkers))
}

/// The bucket of the DEM tile `id` shows.
fn dem<'a>(tiles: &'a OneTile, id: &OverscaledTileID) -> Option<&'a DemContents> {
    match tiles.tile_at(id).contents() {
        Some(Contents::Dem(bucket)) => Some(bucket),
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
    let dem = DemData::new(&PremultipliedImage::new(Size::new(16, 16)), RasterEncoding::Mapbox).expect("DemData");
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

// A DEM decodes in the encoding that its load delivered with it, which each
// load names anew.
#[test]
fn a_dem_decodes_in_the_encoding_delivered_with_it() {
    let test = RasterTileTest::new();
    let mut tiles = test.pool_tiles(DEM);
    let id = tiles.id;
    for encoding in [RasterEncoding::Terrarium, RasterEncoding::Mapbox] {
        tiles.with(|tile| tile.take_image(Some((png(16, 16), Some(encoding))), false));
        tiles.settle(&[id]);
        assert_eq!(dem(&tiles, &id).expect("a DEM").dem().encoding, encoding);
    }
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
        let bucket = dem(&tiles, id).expect("a bucket");
        let dem = bucket.dem();
        assert_eq!((dem.dim, dem.stride, dem.encoding), (16, 18, RasterEncoding::Mapbox));
    }

    let (left, right) = (dem(&tiles, &ids[0]).unwrap(), dem(&tiles, &ids[1]).unwrap());
    for y in 0..16 {
        assert_eq!(left.dem().get(16, y), right.dem().get(0, y));
        assert_eq!(right.dem().get(-1, y), left.dem().get(15, y));
    }

    // The DemData constructor's error crosses as the tile's.
    tiles.set_image(&ids[1], Some(png(16, 8)));
    tiles.settle(&ids[1..]);
    assert_eq!(tiles.errors(), ["raster-dem tiles must be square."]);
    assert!(tiles.tile_at(&ids[1]).is_renderable());
}

// What the layers make of a DEM goes when its border or the part of the
// tile it draws changes: its contents take a new identity, and keep it
// through a mask that changes nothing. Its texture, which the contents make
// once they upload, goes with its border.
#[test]
fn contents_take_a_new_identity_when_their_border_or_mask_changes() {
    let mut context = crate::gfx::context::Resources::detached();
    let image = PremultipliedImage::new(Size::new(16, 16));
    let mut contents = DemContents::new(DemData::new(&image, RasterEncoding::Mapbox).expect("DemData"));
    contents.upload(&mut context);
    let texture = contents.texture().cloned().expect("a texture once uploaded");
    let first = contents.id();
    contents.set_mask([mln_tile_id::CanonicalTileID::new(0, 0, 0)].into());
    assert_eq!(first, contents.id(), "the whole tile, as before");
    contents.set_mask([mln_tile_id::CanonicalTileID::new(1, 0, 0)].into());
    let masked = contents.id();
    assert_ne!(first, masked);
    contents.unmask();
    assert_ne!(masked, contents.id());
    assert_eq!(Some(&texture), contents.texture(), "the masks keep the texture");

    let border = DemContents::new(DemData::new(&image, RasterEncoding::Mapbox).expect("DemData"));
    let (mut tile, mut border) = (Contents::Dem(contents), Contents::Dem(border));
    let before = [&tile, &border].map(|contents| match contents {
        Contents::Dem(dem) => dem.id(),
        _ => unreachable!(),
    });
    crate::tile::dem::backfill_pair(&mut tile, &mut border, crate::tile::dem::Offset { dx: 1, dy: 0 });
    let after = [&tile, &border].map(|contents| match contents {
        Contents::Dem(dem) => dem.id(),
        _ => unreachable!(),
    });
    assert!(before.iter().zip(&after).all(|(before, after)| before != after));
    let Contents::Dem(tile) = &mut tile else { unreachable!() };
    assert!(tile.texture().is_none(), "the border went into the image");
    tile.upload(&mut context);
    assert!(tile.texture().is_some_and(|new| *new != texture));
}
