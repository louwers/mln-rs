//! Hillshades drawn over DEM tiles that the tests make.

use std::sync::Arc;

use mln_engine::storage::Response;
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::support::still::{StillMap, pixel, serve};

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
