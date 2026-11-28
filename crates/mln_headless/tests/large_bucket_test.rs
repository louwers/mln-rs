// Buckets of more vertices than a draw's base vertex reaches
// (`mln_gfx::gpu::MAX_BASE_VERTEX`) draw every segment.

use std::sync::Arc;

use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_headless::HeadlessFrontend;
use mln_style::error::ErrorKind;
use mln_style::size::Size;
use serde_json::json;

const SIZE: u32 = 256;

/// Points that zigzag between latitudes `south` and `north` from longitude
/// -170 to 170, `count` of them, no two in a row alike.
fn zigzag(count: usize, south: f64, north: f64) -> Vec<[f64; 2]> {
    (0..count)
        .map(|i| {
            let lng = -170.0 + 340.0 * i as f64 / count as f64;
            [lng, if i % 2 == 0 { south } else { north }]
        })
        .collect()
}

/// A red line and a blue fill with a green outline, each of `count`
/// vertices, on white: the line north of the equator, the fill south of it.
fn style(count: usize) -> String {
    let line = zigzag(count, 20.0, 60.0);
    let mut ring = zigzag(count, -20.0, -30.0);
    ring.extend([[170.0, -60.0], [-170.0, -60.0], ring[0]]);
    let source = |geometry| json!({"type": "geojson", "maxzoom": 0, "tolerance": 0, "data": geometry});
    json!({
        "version": 8,
        "sources": {
            "line": source(json!({"type": "LineString", "coordinates": line})),
            "fill": source(json!({"type": "Polygon", "coordinates": [ring]})),
        },
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "white"}},
            {"id": "fill", "type": "fill", "source": "fill", "paint": {"fill-color": "blue", "fill-outline-color": "green"}},
            {"id": "line", "type": "line", "source": "line", "paint": {"line-color": "red", "line-width": 2}},
        ],
    })
    .to_string()
}

#[test]
fn a_bucket_past_the_largest_base_vertex_draws() {
    let size = Size::new(SIZE, SIZE);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let device = match mln_headless::harness_device() {
        Ok(device) => device,
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no device: {error}");
            return;
        }
        Err(error) => panic!("a device: {error}"),
    };
    let mut frontend =
        HeadlessFrontend::with_device(device, size, 1.0, Arc::default(), &run_loop, &pool).expect("a frontend");
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, mln_builtins::registry(), &pool).unwrap();
    frontend.attach(&mut map);
    // Line vertices, fill vertices, and the outlines' quads, one per fill
    // vertex, each go past twice the largest base vertex.
    map.load_style_json(&style(70_000));
    let image = frontend.render(&mut map).expect("the still image").image;

    let (rows, _) = image.data.as_chunks::<{ SIZE as usize * 4 }>();
    let red = |pixel: &[u8; 4]| pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60;
    let blue = |pixel: &[u8; 4]| pixel[2] > 200 && pixel[0] < 60 && pixel[1] < 60;
    // The east end of each, which the last segments draw.
    let east = |row: &[u8; SIZE as usize * 4], is: &dyn Fn(&[u8; 4]) -> bool| {
        row[SIZE as usize * 4 * 3 / 4..].as_chunks::<4>().0.iter().any(is)
    };
    let (north, south) = rows.split_at(SIZE as usize / 2);
    assert!(north.iter().any(|row| east(row, &red)), "the line's east end");
    assert!(south.iter().any(|row| east(row, &blue)), "the fill's east end");
}
