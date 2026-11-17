//! The camera paths of the Frame scenarios: a loaded map's camera as it pans,
//! turns and tilts over tiles already there. `mln-bench` draws each path
//! once, headless; the desktop example's benchmark mode draws it in a window,
//! lap after lap.

use mln_map::CameraOptions;
use mln_util::geo::LatLng;
use mln_util::size::Size;

use crate::{Snapshot, Tiles};

/// The view the paths are drawn in: two 512-pixel tiles across, the widest
/// view whose camera, centered in a tile set's area, sees only tiles of the
/// set.
pub const VIEW: Size = Size { width: 1024, height: 768 };

/// The frames of a path: two seconds at 60 Hz, which places symbols about
/// seven times and fades them in and out between placements.
pub const FRAMES: usize = 120;

/// A camera path, with the style it's drawn over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    /// Western San Francisco (the Sunset District) at zoom 13.5: the camera
    /// turns a full circle, pans around a small circle and tilts to 10
    /// degrees and back. It circles the corner where four tiles of zoom 13
    /// meet, and its view stays inside them; at a steeper tilt it would see
    /// more. It ends where it starts.
    Street,
    /// The whole globe (zoom 1.5) turning 120 degrees of longitude under the
    /// camera, which turns and tilts to 30 degrees and back.
    Globe,
    /// The whole world on a flat map at zoom 2, its center on 20 degrees
    /// north swinging from 140 degrees east across the antimeridian to 140
    /// degrees west and back: each lap crosses it twice, and each crossing
    /// moves the tiles in view to other world copies. It ends where it
    /// starts.
    Antimeridian,
}

impl Path {
    /// The path named `name`: `street`, `globe` or `antimeridian`.
    pub fn parse(name: &str) -> Result<Path, String> {
        match name {
            "street" => Ok(Path::Street),
            "globe" => Ok(Path::Globe),
            "antimeridian" => Ok(Path::Antimeridian),
            _ => Err(format!("no camera path {name:?}: street, globe or antimeridian")),
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Path::Street => "street",
            Path::Globe => "globe",
            Path::Antimeridian => "antimeridian",
        }
    }

    /// The style the path is drawn over, as JSON.
    #[must_use]
    pub fn style(self, snapshot: &Snapshot) -> String {
        match self {
            Path::Street => snapshot.style(Tiles::Street),
            Path::Globe => snapshot.globe_style(),
            Path::Antimeridian => snapshot.style(Tiles::World),
        }
    }

    /// The camera of each of the path's [`FRAMES`] frames, by its index.
    pub fn camera(self, snapshot: &Snapshot) -> impl Fn(usize) -> CameraOptions + use<> {
        let corner = match self {
            Path::Street => {
                let (x, y) = tile_at(12, snapshot.area_center(Tiles::Street));
                tile_center(12, x, y)
            }
            Path::Globe | Path::Antimeridian => LatLng::default(),
        };
        move |frame| {
            let t = frame as f64 / FRAMES as f64;
            match self {
                Path::Street => {
                    let turn = std::f64::consts::TAU * t;
                    CameraOptions {
                        center: Some(
                            LatLng::new(
                                corner.latitude() + 0.002 * turn.sin(),
                                corner.longitude() + 0.003 * turn.cos(),
                            )
                            .expect("a center"),
                        ),
                        zoom: Some(13.5),
                        bearing: Some(360.0 * t),
                        pitch: Some(10.0 * (std::f64::consts::PI * t).sin()),
                        ..Default::default()
                    }
                }
                Path::Globe => {
                    let wave = (std::f64::consts::PI * t).sin();
                    CameraOptions {
                        center: Some(LatLng::new(20.0 + 15.0 * wave, -60.0 + 120.0 * t).expect("a center")),
                        zoom: Some(1.5),
                        bearing: Some(30.0 * t),
                        pitch: Some(30.0 * wave),
                        ..Default::default()
                    }
                }
                Path::Antimeridian => {
                    let longitude = 180.0 + 40.0 * (std::f64::consts::TAU * t).sin();
                    let longitude = if longitude >= 180.0 { longitude - 360.0 } else { longitude };
                    CameraOptions {
                        center: Some(LatLng::new(20.0, longitude).expect("a center")),
                        zoom: Some(2.0),
                        ..Default::default()
                    }
                }
            }
        }
    }
}

/// The center of the 512-pixel tile `z`/`x`/`y` of Web Mercator.
#[must_use]
pub fn tile_center(z: u8, x: u32, y: u32) -> LatLng {
    let tiles = f64::from(1u32 << z);
    let longitude = (f64::from(x) + 0.5) / tiles * 360.0 - 180.0;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(y) + 0.5) / tiles)).sinh().atan().to_degrees();
    LatLng::new(latitude, longitude).expect("a tile's center")
}

/// The tile of zoom `z` that holds `point`.
#[must_use]
pub fn tile_at(z: u8, point: LatLng) -> (u32, u32) {
    let tiles = f64::from(1u32 << z);
    let x = (point.longitude() + 180.0) / 360.0 * tiles;
    let latitude = point.latitude().to_radians();
    let y = (1.0 - latitude.tan().asinh() / std::f64::consts::PI) / 2.0 * tiles;
    (x.floor() as u32, y.floor() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_holds_its_center() {
        for (z, x, y) in [(0, 0, 0), (14, 2620, 6332), (6, 10, 24)] {
            assert_eq!(tile_at(z, tile_center(z, x, y)), (x, y));
        }
    }

    #[test]
    fn paths_name_themselves() {
        for path in [Path::Street, Path::Globe, Path::Antimeridian] {
            assert_eq!(Path::parse(path.name()), Ok(path));
        }
        assert!(Path::parse("moon").is_err());
    }
}
