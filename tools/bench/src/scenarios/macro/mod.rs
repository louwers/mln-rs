//! One scenario per thing a user waits for or feels, on a whole map over
//! the fixtures.

pub(crate) mod frame;
pub(crate) mod load;
pub(crate) mod tile_server;

use mln_map::CameraOptions;
use mln_util::geo::LatLng;
use mln_util::size::Size;

use crate::support::fixtures::{Fixtures, Tiles};

/// The view of the continuous scenarios: two 512-pixel tiles across, the
/// widest view whose camera, centered in a tile set's area, sees only tiles
/// of the set.
pub(crate) const VIEW: Size = Size { width: 1024, height: 768 };

/// The center of the area of `tiles`.
pub(crate) fn area_center(fixtures: &Fixtures, tiles: Tiles) -> LatLng {
    let [west, south, east, north] = fixtures.tile_set(tiles).area;
    LatLng::new(f64::midpoint(south, north), f64::midpoint(west, east)).expect("an area's center")
}

/// A camera at `zoom` over the center of the street tiles.
pub(crate) fn street_center(fixtures: &Fixtures, zoom: f64) -> CameraOptions {
    CameraOptions { center: Some(area_center(fixtures, Tiles::Street)), zoom: Some(zoom), ..Default::default() }
}

/// The style over the world's tiles, on the globe.
pub(crate) fn globe_style(fixtures: &Fixtures) -> String {
    let mut style = fixtures.style_value(Tiles::World);
    style["projection"] = serde_json::json!({ "type": "globe" });
    style.to_string()
}
