//! The `stamp` source type and its two layer types through the facade: a
//! value that the source's origin makes for each tile, which both layer
//! types view and draw from, as the source contract and the layer contract
//! say.

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;

use mln_extension_tests::stamp::{Seen, StampFillLayer, StampMarkLayer, StampSource};
use mln_rs::layer::{
    Draws, Layer, LayerTypeInfo, NewCx, NoProperties, PrepareCx, SourceKind, StyleLayerType, ValueKind, ValueTiles,
};
use mln_rs::mln_map_types::{CameraOptions, CameraUpdate};
use mln_rs::mln_render::{Registry, RegistryBuilder};
use mln_rs::mln_util::ErrorKind;
use mln_rs::mln_util::geo::LatLng;
use mln_rs::mln_util::image::PremultipliedImage;
use mln_rs::source::{Edit, OverscaledTileID};
use support::{Fixture, SIZE, wait};

/// A white map at zoom 0 over a stamp source of 32-pixel tiles, which the
/// two layers view: the map shows four tiles of zoom 4, each a quarter of
/// it.
const STYLE: &str = r##"{"version":8,"transition":{"duration":0},
    "sources":{"stamps":{"type":"stamp"}},
    "layers":[{"id":"background","type":"background","paint":{"background-color":"#ffffff"}},
        {"id":"fill","type":"stamp-fill","source":"stamps"},
        {"id":"mark","type":"stamp-mark","source":"stamps"}]}"##;

fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    mln_extension_tests::stamp::register(&mut builder).unwrap();
    builder.build()
}

/// What the stamp layers made their tiles' values of, oldest first, the
/// fill's and the mark's.
fn made(fixture: &Fixture) -> [Vec<Seen>; 2] {
    let fill = fixture.session.query_rendered_layer::<StampFillLayer, _>("fill", |layer, _| Ok(layer.made()));
    let mark = fixture.session.query_rendered_layer::<StampMarkLayer, _>("mark", |layer, _| Ok(layer.made()));
    [wait(fill), wait(mark)]
}

/// The stamps of the tiles that the last frame drew, in order, the fill's
/// and the mark's.
fn drawn(fixture: &Fixture) -> [BTreeSet<(OverscaledTileID, usize, u32)>; 2] {
    let set = |seen: Vec<Seen>| seen.into_iter().map(|seen| (seen.tile, seen.stamp, seen.version)).collect();
    let fill = fixture.session.query_rendered_layer::<StampFillLayer, _>("fill", |layer, _| Ok(layer.drawn()));
    let mark = fixture.session.query_rendered_layer::<StampMarkLayer, _>("mark", |layer, _| Ok(layer.drawn()));
    [set(wait(fill)), set(wait(mark))]
}

/// The tiles that `drawn` holds.
fn tiles(drawn: &BTreeSet<(OverscaledTileID, usize, u32)>) -> BTreeSet<OverscaledTileID> {
    drawn.iter().map(|(tile, ..)| *tile).collect()
}

/// The zoom-`z` tiles at `xs` and `ys`, of the world copy `wrap`.
fn ids(z: u8, wrap: i16, xs: &[u32], ys: &[u32]) -> BTreeSet<OverscaledTileID> {
    xs.iter().flat_map(|&x| ys.iter().map(move |&y| OverscaledTileID::with_zxy(z, wrap, z, x, y))).collect()
}

/// Changes the stamp source as `change` says.
fn edit_stamps(fixture: &Fixture, change: impl FnOnce(&mut mln_rs::source::SourceData<StampSource>) + Send + 'static) {
    wait(fixture.map.apply_style_edit(Edit::update_source::<StampSource>("stamps", "a stamp source", |data| {
        change(data);
        true
    })));
}

fn move_camera(fixture: &Fixture, camera: CameraOptions) {
    wait(fixture.map.update_camera(CameraUpdate { camera, ..Default::default() }));
}

fn center(longitude: f64) -> CameraOptions {
    CameraOptions { center: LatLng::new(0.0, longitude).ok(), ..Default::default() }
}

/// The premultiplied RGBA of the pixel at `x`, `y` of `image`.
fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * SIZE.width + x) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

/// Asserts that `image` is the expected image `name` to within 2/255 a
/// channel. With `MLN_UPDATE_EXPECTED` set, writes it as that image instead.
fn assert_expected(image: &PremultipliedImage, name: &str) {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("tests/expected").join(name);
    if std::env::var_os("MLN_UPDATE_EXPECTED").is_some() {
        std::fs::write(&path, mln_rs::mln_image::encode_png(image)).unwrap();
        return;
    }
    let expected = mln_rs::mln_image::decode_png(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(expected.size, image.size);
    let distance = image.data.iter().zip(&expected.data).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert!(distance <= 2, "the image differs from {} by {distance}/255", path.display());
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 128, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

/// Each tile's stamp, made once, reaches both layers as the same value:
/// the fill fills the tile with its fill, and the mark draws a square of its
/// mark in the tile's middle.
#[test]
fn two_layer_types_draw_from_each_tiles_one_value() {
    let fixture = Fixture::new(registry(), STYLE);
    let image = wait(fixture.session.read_texture());
    assert_expected(&image, "stamp.png");
    // The top left tile, 7/7, fills yellow around its green mark.
    assert_eq!(YELLOW, pixel(&image, 2, 2));
    assert_eq!(GREEN, pixel(&image, 16, 16));

    let [fill, mark] = drawn(&fixture);
    assert_eq!(tiles(&fill), ids(4, 0, &[7, 8], &[7, 8]));
    assert_eq!(fill, mark, "both layers draw each tile from the same value");
    let [fill, mark] = made(&fixture);
    assert_eq!(fill.len(), 4, "the fill made each tile's value once");
    assert_eq!(fill, mark);
}

/// A new version of the source gives each tile a new stamp, which both
/// layers make their tiles' values of again.
#[test]
fn a_new_value_reaches_both_layers() {
    let fixture = Fixture::new(registry(), STYLE);
    let [before, _] = drawn(&fixture);

    edit_stamps(&fixture, |data| data.options.version = 1);
    fixture.render_loaded();
    let [fill, mark] = drawn(&fixture);
    assert_eq!(fill, mark);
    assert!(fill.iter().all(|&(_, _, version)| version == 1), "{fill:?}");
    assert_eq!(tiles(&fill), tiles(&before));
    let [fill, mark] = made(&fixture);
    assert_eq!(fill.len(), 8, "the fill made each tile's value again");
    assert_eq!(fill, mark);
    // The top left tile's colors moved on by one.
    let image = wait(fixture.session.read_texture());
    assert_eq!(RED, pixel(&image, 2, 2));
}

/// While a tile waits for its value, a parent with one draws in its place,
/// under the children that have theirs once they come.
#[test]
fn a_parent_draws_while_its_children_wait() {
    let fixture = Fixture::new(registry(), STYLE);
    let [parents, _] = drawn(&fixture);
    edit_stamps(&fixture, |data| data.options.hold = true);
    move_camera(&fixture, CameraOptions { zoom: Some(1.0), ..Default::default() });
    fixture.render();
    let [fill, mark] = drawn(&fixture);
    assert_eq!(fill, parents, "the parents draw, with the values they had");
    assert_eq!(fill, mark);

    edit_stamps(&fixture, |data| data.options.hold = false);
    fixture.render_loaded();
    let [fill, mark] = drawn(&fixture);
    assert_eq!(tiles(&fill), ids(5, 0, &[15, 16], &[15, 16]));
    assert_eq!(fill, mark);
}

/// A tile in a world copy that the map comes to shows the value of the same
/// tile in the world copy it showed before, at once, although the source
/// holds new tiles' values; a new tile without a copy waits for its own.
#[test]
fn a_new_world_copy_shows_the_value_of_another_at_once() {
    let fixture = Fixture::new(registry(), STYLE);
    // In steps of less than half the world, which leave each tile in its
    // world copy: 170 degrees west shows tile 15 west of the antimeridian,
    // and tiles 0 and 1 east of it.
    move_camera(&fixture, center(-170.0));
    fixture.render_loaded();
    let [west, _] = drawn(&fixture);
    let west_ids: BTreeSet<_> = ids(4, -1, &[15], &[7, 8]).union(&ids(4, 0, &[0, 1], &[7, 8])).copied().collect();
    assert_eq!(tiles(&west), west_ids);

    edit_stamps(&fixture, |data| data.options.hold = true);
    move_camera(&fixture, center(0.0));
    fixture.render();
    // 170 degrees east shows tiles 14 and 15 west of the antimeridian, and
    // tile 0 east of it: 15 and 0 in the next world copy east of those the
    // map showed at 170 degrees west, and 14 in none.
    move_camera(&fixture, center(170.0));
    fixture.render();
    let [fill, mark] = drawn(&fixture);
    assert_eq!(fill, mark);
    let copies: BTreeSet<_> = west
        .iter()
        .filter(|(tile, ..)| tile.canonical.x != 1)
        .map(|&(tile, stamp, version)| (tile.unwrap_to(tile.wrap + 1), stamp, version))
        .collect();
    assert_eq!(fill, copies, "each tile with a copy shows its copy's value, and tile 14 none");
}

/// The style half of a layer type that views tiles of another value than
/// stamps.
struct Other;

impl StyleLayerType for Other {
    const INFO: LayerTypeInfo =
        LayerTypeInfo { name: "other", source: SourceKind::Value(ValueKind::of::<u32>()), from_json: true };
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

/// A layer of [`Other`], which draws nothing.
struct OtherLayer;

impl Layer for OtherLayer {
    type Style = Other;
    type Input = ValueTiles<u32>;
    type Tile = ();

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        OtherLayer
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, _out: &mut Draws) {}
}

/// A layer that views another kind of tiles than its source's, or values
/// of another type, is left out as the style loads, with an error, and
/// can't be added.
#[test]
fn a_layer_of_another_kind_than_its_source_is_refused() {
    let style = r##"{"version":8,
        "sources":{"stamps":{"type":"stamp"},
            "points":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},
        "layers":[{"id":"background","type":"background","paint":{"background-color":"#ffffff"}},
            {"id":"circles","type":"circle","source":"stamps"},
            {"id":"fill","type":"stamp-fill","source":"points"},
            {"id":"other","type":"other","source":"stamps"},
            {"id":"mark","type":"stamp-mark","source":"stamps"}]}"##;
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    mln_extension_tests::stamp::register(&mut builder).unwrap();
    builder.add_layer_type::<OtherLayer>().unwrap();
    let fixture = Fixture::new(builder.build(), style);
    let layers: Vec<String> = wait(fixture.map.style_layers()).into_iter().map(|layer| layer.id).collect();
    assert_eq!(layers, ["background", "mark"]);

    for layer in [
        r#"{"id":"circles","type":"circle","source":"stamps"}"#,
        r#"{"id":"fill","type":"stamp-fill","source":"points"}"#,
        r#"{"id":"other","type":"other","source":"stamps"}"#,
    ] {
        let error = fixture.map.add_style_layer_json(layer, None).unwrap().wait().unwrap_err();
        assert_eq!(ErrorKind::InvalidArgument, error.kind, "{layer}: {error}");
    }
}

/// A runtime without the stamp types draws the rest of a style that has
/// them.
#[test]
fn a_style_with_stamps_draws_without_their_types() {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    let fixture = Fixture::new(builder.build(), STYLE);
    let image = wait(fixture.session.read_texture());
    assert!(image.data.iter().all(|&channel| channel == 255), "the background alone");
}
