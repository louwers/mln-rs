//! The tiles and symbols of maps drawn as a globe.

use std::sync::Arc;
use std::time::Duration;

use mln_engine::TileOperation;
use mln_engine::camera::{CameraOptions, CameraUpdate, CameraUpdateMode};
use mln_engine::map::{MapEvent, MapMode};
use mln_engine::storage::Response;
use mln_engine::tile::OverscaledTileID;
use mln_runtime::{RuntimeEventKind, RuntimeOptions};
use mln_style::TransitionOptions;
use mln_style::bitmap::PremultipliedImage;
use mln_style::geo::LatLng;
use mln_style::size::Size;

use crate::support::still::{StillMap, StillOptions, assert_near, fixtures_root, read_bytes, read_file, serve};
use crate::support::wait;

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).expect("LatLng")
}

/// The tiles of `source`, or of every source, that the map started to parse
/// after the first `from` of its events.
fn parsed_since(map: &StillMap, from: usize, source: Option<&str>) -> Vec<OverscaledTileID> {
    map.events.events()[from..]
        .iter()
        .filter_map(|event| match &event.kind {
            RuntimeEventKind::Map(MapEvent::TileAction { op: TileOperation::StartParse, id, source_id })
                if source.is_none_or(|source| source == source_id) =>
            {
                Some(*id)
            }
            _ => None,
        })
        .collect()
}

/// Renders a still image, and returns the tiles of `source`, or of every
/// source, that the map started to parse for it.
fn render_parsing(map: &StillMap, source: Option<&str>) -> Vec<OverscaledTileID> {
    let from = map.events.events().len();
    map.render();
    parsed_since(map, from, source)
}

fn globe_rendering(map: &StillMap) -> bool {
    map.map.snapshot().expect("a snapshot").globe_rendering
}

/// Crossing the globe's hand-off to Mercator parses none of the tiles
/// already on screen again.
#[test]
fn the_globes_hand_off_keeps_its_tiles() {
    let map = StillMap::new(StillOptions {
        size: Size::new(512, 512),
        runtime_options: RuntimeOptions {
            asset_path: Some(fixtures_root().join("test/fixtures/api/assets")),
            ..RuntimeOptions::default()
        },
        ..StillOptions::default()
    });
    map.load_json(&read_file("test/fixtures/api/water.json"));
    wait(map.map.set_style_root_property("projection", r#"{"type":"globe"}"#));
    // The fixtures have tile 10/163/395.
    let center = ll(37.8, -122.5);
    map.jump(CameraOptions { center: Some(center), zoom: Some(10.5), ..Default::default() });
    let first_visit = render_parsing(&map, Some("mapbox"));
    assert!(globe_rendering(&map));
    assert_ne!(first_visit, Vec::<OverscaledTileID>::new());

    // Zoom 12.5 is Mercator. The zoom 10 tiles stay on screen while their
    // children load.
    map.jump(CameraOptions { center: Some(center), zoom: Some(12.5), ..Default::default() });
    let parsed = render_parsing(&map, Some("mapbox"));
    assert!(!globe_rendering(&map));
    let reparsed: Vec<&OverscaledTileID> = parsed.iter().filter(|id| first_visit.contains(id)).collect();
    assert!(
        reparsed.is_empty(),
        "{} of {} tiles were parsed again after the hand-off",
        reparsed.len(),
        first_visit.len()
    );
}

/// The globe's zoom 0 tile takes the wrap of the world copy that the center
/// is in, so it changes wrap whenever the center crosses the antimeridian.
/// It is the same piece of the sphere in either, and isn't parsed again.
#[test]
fn the_globe_keeps_its_tiles_across_the_antimeridian() {
    let map = StillMap::new(StillOptions { size: Size::new(512, 512), ..StillOptions::default() });
    let ocean = Arc::new(read_bytes("test/fixtures/map/globe_tile_masks/ocean.mvt"));
    serve(&map.runtime, move |_| Response { data: Some(Arc::clone(&ocean)), ..Response::default() });
    map.load_json(
        r#"{"version": 8, "projection": {"type": "globe"},
            "sources": {"ocean": {"type": "vector", "tiles": ["a/{z}/{x}/{y}"]}},
            "layers": [{"id": "ocean", "type": "fill", "source": "ocean", "source-layer": "water",
                        "paint": {"fill-color": "blue"}}]}"#,
    );

    map.jump(CameraOptions { center: Some(ll(0.0, 179.0)), zoom: Some(0.0), ..Default::default() });
    let parsed = render_parsing(&map, None);
    assert!(globe_rendering(&map));
    assert_ne!(parsed, Vec::<OverscaledTileID>::new());
    for id in &parsed {
        assert_eq!(OverscaledTileID::with_zxy(0, 0, 0, 0, 0), *id);
    }

    map.jump(CameraOptions { center: Some(ll(0.0, 181.0)), ..Default::default() });
    let parsed = render_parsing(&map, None);
    // The map reports the center in the world copy that it wraps to.
    let center = map.map.snapshot().expect("a snapshot").view.camera.center;
    let turns = (center.longitude() - 181.0) / 360.0;
    assert_near(turns, turns.round(), 1e-9);
    assert!(parsed.is_empty(), "{} tiles parsed again, the first {}", parsed.len(), parsed[0]);
}

/// A screen-aligned icon hides once the globe turns its anchor behind the
/// planet, in the first frame after the turn.
#[test]
fn the_globe_hides_a_screen_aligned_icon_once_its_anchor_is_behind_the_planet() {
    const SIDE: u32 = 24;

    let map = StillMap::new(StillOptions { mode: MapMode::Continuous, ..StillOptions::default() });
    map.load_json(
        r#"{"version": 8, "projection": {"type": "vertical-perspective"},
            "sources": {"point": {"type": "geojson", "data": {"type": "Point", "coordinates": [30, 0]}}},
            "layers": [{"id": "background", "type": "background", "paint": {"background-color": "white"}},
                       {"id": "icon", "type": "symbol", "source": "point",
                        "layout": {"icon-image": "square", "icon-allow-overlap": true}}]}"#,
    );
    let red = PremultipliedImage::from_vec(Size::new(SIDE, SIDE), [255, 0, 0, 255].repeat((SIDE * SIDE) as usize));
    let square =
        mln_style::image::Impl::new("square".to_owned(), red, 1.0, false, Vec::new(), Vec::new(), None, None, None)
            .expect("a style image");
    wait(map.map.set_style_image(square));
    map.jump(CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(1.0), ..Default::default() });

    let red_pixels = |map: &StillMap| {
        let image = map.image();
        image.data.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60).count()
    };
    map.settle();
    assert!(red_pixels(&map) > (SIDE * SIDE / 2) as usize);

    // Turn the planet so that the icon's anchor is 130 degrees from the
    // center. A long fade keeps placement showing it at full opacity on the
    // next frame, so only the horizon test in the shader can hide it there.
    wait(map.map.set_style_transition_options(TransitionOptions {
        duration: Some(Duration::from_secs(30)),
        ..TransitionOptions::default()
    }));
    let camera = CameraOptions { center: Some(ll(0.0, -100.0)), ..Default::default() };
    let turned =
        wait(map.map.update_camera(CameraUpdate { mode: CameraUpdateMode::Jump, camera, ..CameraUpdate::default() }));
    let frame = map.frame();
    assert!(frame.map_update_generation >= turned.render_update_generation);
    assert_eq!(0, red_pixels(&map));
}
