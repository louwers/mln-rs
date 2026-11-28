//! Hillshades drawn over DEM tiles that the tests make.

use std::sync::Arc;

use mln_engine::RenderMode;
use mln_engine::camera::CameraOptions;
use mln_engine::map::{MapEvent, MapMode, RenderFrameStatus};
use mln_engine::storage::Response;
use mln_runtime::{DemandMode, FrameDisposition, FrameResult, RuntimeEventKind};
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::support::still::{StillMap, StillOptions, pixel, serve};

const SIDE: u32 = 256;

/// A DEM tile of 256 pixels a side, in the Mapbox encoding, of a plane that
/// rises to the north, 1 m for every 4 m of the world's width at the equator,
/// from 1000 m at the equator. Its green byte carries every few rows.
fn northward_slope_png(z: i8, y: i32) -> Vec<u8> {
    const EQUATOR_TO_POLE: f64 = 40_075_016.685_578_49 / 2.0;
    let tiles = f64::from(1u32 << z);
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let y = (f64::from(y) + ((index / 256) as f64 + 0.5) / 256.0) / tiles;
        let height = 1000.0 + (0.5 - y) * EQUATOR_TO_POLE * 2.0 / 4.0;
        let value = ((height + 10000.0) * 10.0).round() as u32;
        *pixel = [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255];
    }
    mln_engine::image::encode_png(&image)
}

/// A hillshade shades a plane slope evenly, seen past its DEM's deepest
/// zoom: a backend that read the DEM's bytes at too low a precision would
/// draw bands where the green byte carries.
#[test]
fn a_hillshade_shades_a_plane_slope_evenly() {
    let style = r#"{
        "version": 8,
        "center": [0.001, 0.001],
        "zoom": 16,
        "sources": {
            "dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 256, "minzoom": 13, "maxzoom": 13}
        },
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "white"}},
            {"id": "hillshade", "type": "hillshade", "source": "dem"}
        ]
    }"#;
    let map = StillMap::still();
    serve(&map.runtime, |resource| {
        let tile = resource.tile_data.as_ref().expect("a tile");
        Response { data: Some(Arc::new(northward_slope_png(tile.z, tile.y))), ..Response::default() }
    });
    map.load_json(style);
    let image = map.render();

    let shade = pixel(&image, SIDE / 2, SIDE / 2);
    let uneven: Vec<(u32, u32)> = (0..SIDE)
        .flat_map(|y| (0..SIDE).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(&image, x, y).iter().zip(shade).any(|(a, b)| a.abs_diff(b) > 2))
        .collect();
    assert!(
        uneven.is_empty(),
        "{} pixels shade the slope otherwise than {shade:?}, as at {:?}",
        uneven.len(),
        uneven.first()
    );
}

/// A DEM tile of 256 pixels a side, in the Terrarium encoding, that rises
/// to the east.
fn eastward_slope_png() -> Arc<Vec<u8>> {
    let mut dem = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in dem.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        pixel.copy_from_slice(&[128, (index % 256) as u8, 0, 255]);
    }
    Arc::new(mln_engine::image::encode_png(&dem))
}

/// A hillshade lit from the viewport, as its lights are by default, of
/// [`eastward_slope_png`]'s tiles.
const EASTWARD_SLOPE_STYLE: &str = r#"{
    "version": 8,
    "zoom": 2,
    "sources": {"dem": {"type": "raster-dem", "encoding": "terrarium", "tiles": ["dem/{z}/{x}/{y}.png"], "maxzoom": 2}},
    "layers": [{"id": "hillshade", "type": "hillshade", "source": "dem", "paint": {"hillshade-exaggeration": 1}}]
}"#;

/// A 64 by 64 map in `mode` of [`EASTWARD_SLOPE_STYLE`].
fn eastward_slope_map(mode: MapMode) -> StillMap {
    let map = StillMap::new(StillOptions { size: Size::new(64, 64), mode, ..StillOptions::default() });
    let dem = eastward_slope_png();
    serve(&map.runtime, move |_| Response { data: Some(Arc::clone(&dem)), ..Response::default() });
    map.load_json(EASTWARD_SLOPE_STYLE);
    map
}

/// A hillshade lit from the viewport turns its lights with the map: after
/// a rotation it draws as a map that starts rotated does.
#[test]
fn a_viewport_lit_hillshade_turns_its_lights_with_the_map() {
    // The still of a map whose camera turned to each of `bearings` in turn,
    // with a still at each.
    let render = |bearings: &[f64]| {
        let map = eastward_slope_map(MapMode::Static);
        let mut image = None;
        for &bearing in bearings {
            map.jump(CameraOptions { bearing: Some(bearing), ..CameraOptions::default() });
            image = Some(map.render());
        }
        image.expect("a bearing")
    };
    let turned = render(&[0.0, 90.0]);
    assert!(render(&[0.0]) != turned, "the hillshade shows its lights' direction");
    assert!(render(&[90.0]) == turned, "the turned hillshade draws otherwise than one that starts turned");
}

/// Draws a frame of `map` in `mode`. Returns the frame's result, and for a
/// frame that rendered, the status that the map reported for it.
fn frame(map: &StillMap, mode: DemandMode) -> (FrameResult, Option<RenderFrameStatus>) {
    let result = map.frame_with(mode);
    if result.disposition != FrameDisposition::Rendered {
        return (result, None);
    }
    // The renderer counts the session's frames as the session does.
    let event = map.events.wait_for(|event| {
        matches!(&event.kind, RuntimeEventKind::Map(MapEvent::RenderFrameFinished(status))
            if u64::try_from(status.rendering_stats.num_frames) == Ok(result.frame_generation))
    });
    let Some(RuntimeEventKind::Map(MapEvent::RenderFrameFinished(status))) = event.map(|event| event.kind) else {
        panic!("no status for frame {}", result.frame_generation);
    };
    (result, Some(status))
}

/// A hillshade prepares each DEM tile's slopes once: the frames after the
/// one that prepared them draw the same image without the prepare draws.
/// C++ draws still images, whose draw calls the runtime doesn't report:
/// here a continuous map draws frames instead.
#[test]
fn a_hillshade_prepares_its_slopes_once() {
    let map = eastward_slope_map(MapMode::Continuous);
    let mut loading = Vec::new();
    loop {
        let (result, status) = frame(&map, DemandMode::WaitForUpdate);
        loading.extend(status);
        if result.mode == RenderMode::Full && !result.needs_repaint {
            break;
        }
    }
    let first = map.image();
    let draws = |status: Option<RenderFrameStatus>| status.expect("a frame").rendering_stats.num_draw_calls;
    let second = draws(frame(&map, DemandMode::Force).1);
    assert!(map.image() == first, "a second frame draws otherwise");
    let third = draws(frame(&map, DemandMode::Force).1);
    assert!(map.image() == first, "a third frame draws otherwise");
    let prepared = loading.iter().map(|status| status.rendering_stats.num_draw_calls).max().unwrap_or_default();
    assert!(second < prepared, "{prepared} draws, then {second}");
    assert_eq!(second, third);
}
