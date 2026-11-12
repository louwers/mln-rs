// A vector source's `emptyResponse`: with `missing`, a tile that answers
// with no content counts as absent and its parent is drawn in its place;
// with `blank`, the default, the tile is drawn empty.

mod support;

use std::sync::Arc;

use mln_storage::{Kind, Resource, Response};
use mln_util::image::PremultipliedImage;
use support::*;

// protobuf encoding for a hand-made tile.
fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn varint_field(out: &mut Vec<u8>, tag: u64, value: u64) {
    varint(out, tag << 3);
    varint(out, value);
}

fn bytes_field(out: &mut Vec<u8>, tag: u64, bytes: &[u8]) {
    varint(out, (tag << 3) | 2);
    varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// A tile whose layer "l" has one polygon over the whole tile and its buffer.
fn full_tile() -> Vec<u8> {
    let mut geometry = Vec::new();
    // MoveTo(-64, -64); LineTo(+4224, 0), (0, +4224), (-4224, 0); ClosePath
    // (zigzag-encoded deltas).
    for value in [9, 127, 127, 26, 8448, 0, 0, 8448, 8447, 0, 15] {
        varint(&mut geometry, value);
    }
    let mut feature = Vec::new();
    varint_field(&mut feature, 3, 3); // POLYGON
    bytes_field(&mut feature, 4, &geometry);

    let mut layer = Vec::new();
    varint_field(&mut layer, 15, 2); // version
    bytes_field(&mut layer, 1, b"l");
    bytes_field(&mut layer, 2, &feature);
    varint_field(&mut layer, 5, 4096); // extent
    let mut tile = Vec::new();
    bytes_field(&mut tile, 3, &layer);
    tile
}

/// Renders a red background under a green fill of a vector source at z1,
/// whose tile 1/0/0 answers with no content and whose other tiles are
/// covered by the fill.
fn render(empty_response: Option<&str>) -> PremultipliedImage {
    let test = MapTest::new();
    let tile = Arc::new(full_tile());
    test.file_source.set_response(Kind::Tile, move |resource: &Resource| {
        let mut response = Response::new();
        let tile_data = resource.tile_data.as_ref().expect("tile data");
        if (tile_data.z, tile_data.x, tile_data.y) == (1, 0, 0) {
            response.no_content = true;
        } else {
            response.data = Some(tile.clone());
        }
        Some(response)
    });
    let empty_response = empty_response.map_or(String::new(), |value| format!(r#", "emptyResponse": "{value}""#));
    test.load_json(&format!(
        r##"{{
  "version": 8,
  "center": [0, 0],
  "zoom": 1,
  "sources": {{ "tiles": {{ "type": "vector", "tiles": ["{{z}}/{{x}}/{{y}}"]{empty_response} }} }},
  "layers": [
    {{ "id": "background", "type": "background", "paint": {{ "background-color": "#f00" }} }},
    {{ "id": "fill", "type": "fill", "source": "tiles", "source-layer": "l",
      "paint": {{ "fill-color": "#0f0", "fill-antialias": false }} }}
  ]
}}"##
    ));
    test.render().image
}

fn pixel(image: &PremultipliedImage, x: usize, y: usize) -> [u8; 4] {
    let offset = y * image.stride() + x * 4;
    image.data[offset..offset + 4].try_into().unwrap()
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// The 256 px view at z1 shows a quarter of each z1 tile; tile 1/0/0 is its
/// top left quarter.
fn assert_quarters(image: &PremultipliedImage, top_left: [u8; 4]) {
    assert_eq!(pixel(image, 64, 64), top_left, "tile 1/0/0");
    assert_eq!(pixel(image, 192, 64), GREEN, "tile 1/1/0");
    assert_eq!(pixel(image, 64, 192), GREEN, "tile 1/0/1");
    assert_eq!(pixel(image, 192, 192), GREEN, "tile 1/1/1");
}

#[test]
fn blank_draws_an_empty_tile() {
    assert_quarters(&render(None), RED);
    assert_quarters(&render(Some("blank")), RED);
}

#[test]
fn missing_draws_the_parent_in_its_place() {
    assert_quarters(&render(Some("missing")), GREEN);
}
