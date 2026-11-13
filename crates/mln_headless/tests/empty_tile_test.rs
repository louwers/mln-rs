// Empty and absent tiles. An empty tile response (HTTP 204) is a tile with nothing in it, which
// draws nothing, unless a vector source is `sparse`. A 404 is a tile the tileset doesn't have: a
// raster tile's parent is drawn in its place, as in GL JS, and a vector tile is empty unless the
// source is `sparse`.

mod support;

use std::sync::Arc;

use mln_storage::response::{Reason, ResourceError};
use mln_storage::{Data, Kind, Resource, Response};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
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

/// A green PNG.
fn green_png() -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(4, 4));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&GREEN);
    }
    mln_image::encode_png(&image)
}

/// How tile 1/0/0 answers.
#[derive(Clone, Copy)]
enum Answer {
    /// HTTP 204.
    NoContent,
    /// HTTP 404, as the HTTP file source reports it for a tile.
    NotFound,
    /// A file source's Not Found error.
    NotFoundError,
}

/// Renders a red background under a green layer of a source at z1 whose
/// tile 1/0/0 answers with `answer` and whose other tiles cover the layer.
/// `source` is the source's type and extra members, `layer` the layer's.
fn render(tile: Data, source: &str, layer: &str, answer: Answer) -> PremultipliedImage {
    let test = MapTest::new();
    test.file_source.set_response(Kind::Tile, move |resource: &Resource| {
        let mut response = Response::new();
        let tile_data = resource.tile_data.as_ref().expect("tile data");
        if (tile_data.z, tile_data.x, tile_data.y) != (1, 0, 0) {
            response.data = Some(Arc::clone(&tile));
            return Some(response);
        }
        match answer {
            Answer::NoContent => response.no_content = true,
            Answer::NotFound => {
                response.no_content = true;
                response.tile_not_found = true;
            }
            Answer::NotFoundError => {
                response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "not found")));
            }
        }
        Some(response)
    });
    test.load_json(&format!(
        r##"{{
  "version": 8,
  "center": [0, 0],
  "zoom": 1,
  "sources": {{ "tiles": {{ {source}, "tiles": ["{{z}}/{{x}}/{{y}}"] }} }},
  "layers": [
    {{ "id": "background", "type": "background", "paint": {{ "background-color": "#f00" }} }},
    {{ "id": "layer", "source": "tiles", {layer} }}
  ]
}}"##
    ));
    test.render().image
}

fn render_vector(sparse: Option<bool>, answer: Answer) -> PremultipliedImage {
    let sparse = sparse.map_or(String::new(), |sparse| format!(r#", "sparse": {sparse}"#));
    let layer = r##""type": "fill", "source-layer": "l", "paint": { "fill-color": "#0f0", "fill-antialias": false }"##;
    render(Arc::new(full_tile()), &format!(r#""type": "vector"{sparse}"#), layer, answer)
}

fn render_raster(answer: Answer) -> PremultipliedImage {
    let layer = r#""type": "raster", "paint": { "raster-fade-duration": 0 }"#;
    render(Arc::new(green_png()), r#""type": "raster", "tileSize": 512"#, layer, answer)
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
fn vector_tiles_are_empty() {
    for answer in [Answer::NoContent, Answer::NotFound, Answer::NotFoundError] {
        assert_quarters(&render_vector(None, answer), RED);
        assert_quarters(&render_vector(Some(false), answer), RED);
    }
}

#[test]
fn a_sparse_vector_source_draws_the_parent_in_their_place() {
    for answer in [Answer::NoContent, Answer::NotFound, Answer::NotFoundError] {
        assert_quarters(&render_vector(Some(true), answer), GREEN);
    }
}

#[test]
fn an_empty_raster_tile_draws_nothing() {
    assert_quarters(&render_raster(Answer::NoContent), RED);
}

#[test]
fn an_absent_raster_tile_draws_the_parent_in_its_place() {
    assert_quarters(&render_raster(Answer::NotFound), GREEN);
    assert_quarters(&render_raster(Answer::NotFoundError), GREEN);
}
