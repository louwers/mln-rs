//! The camera paths of the Frame scenarios: a loaded map's camera as it pans,
//! turns and tilts over tiles already there. `mln-bench` draws each path
//! once, headless; the desktop example's benchmark mode draws it in a window,
//! lap after lap.

use mln_engine::camera::CameraOptions;
use mln_style::geo::LatLng;
use mln_style::size::Size;

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
    /// Downtown San Francisco at zoom 22, eight zooms past its tiles' 14,
    /// tilted to 60 degrees: the camera turns a full circle over the corner
    /// where four tiles of zoom 14 meet, by Union Square.
    Overzoom,
    /// The whole globe (zoom 1.5) turning 120 degrees of longitude under the
    /// camera, which turns and tilts to 30 degrees and back.
    Globe,
    /// The whole world on a flat map at zoom 2, its center on 20 degrees
    /// north swinging from 140 degrees east across the antimeridian to 140
    /// degrees west and back: each lap crosses it twice, and each crossing
    /// moves the tiles in view to other world copies. It ends where it
    /// starts.
    Antimeridian,
    /// The earthquakes of a month over the world's tiles, around Japan: the
    /// camera circles central Japan as it zooms from 5 to 8 and back out, so
    /// the heatmap fades into a circle for each earthquake and back. It ends
    /// where it starts.
    Earthquakes,
    /// San Francisco's hills, tinted by elevation and hillshaded, at zoom 13:
    /// the camera circles Twin Peaks, turning a quarter turn and back, tilted
    /// between 30 and 45 degrees. It ends where it starts.
    Terrain,
    /// The path of [`Path::Terrain`] over [`Snapshot::terrain_3d_style`],
    /// whose ground the terrain tiles raise.
    Terrain3d,
    /// Downtown San Francisco at zoom 16, tilted to 60 degrees, where the
    /// labels are dense: the camera turns a full circle in place over
    /// [`Snapshot::labels_style`], whose POI labels choose among anchors. It
    /// ends where it starts.
    Labels,
}

impl Path {
    /// Every path.
    pub const ALL: [Path; 8] = [
        Path::Street,
        Path::Overzoom,
        Path::Globe,
        Path::Antimeridian,
        Path::Earthquakes,
        Path::Terrain,
        Path::Terrain3d,
        Path::Labels,
    ];

    /// The path named `name`, such as `street`.
    pub fn parse(name: &str) -> Result<Path, String> {
        Path::ALL.into_iter().find(|path| path.name() == name).ok_or_else(|| {
            let names: Vec<&str> = Path::ALL.iter().map(|path| path.name()).collect();
            format!("no camera path {name:?}: {}", names.join(", "))
        })
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Path::Street => "street",
            Path::Overzoom => "overzoom",
            Path::Globe => "globe",
            Path::Antimeridian => "antimeridian",
            Path::Earthquakes => "earthquakes",
            Path::Terrain => "terrain",
            Path::Terrain3d => "terrain-3d",
            Path::Labels => "labels",
        }
    }

    /// The style the path is drawn over, as JSON.
    #[must_use]
    pub fn style(self, snapshot: &Snapshot) -> String {
        match self {
            Path::Street | Path::Overzoom => snapshot.style(Tiles::Street),
            Path::Globe => snapshot.globe_style(),
            Path::Antimeridian => snapshot.style(Tiles::World),
            Path::Earthquakes => snapshot.earthquakes_style(),
            Path::Terrain => snapshot.terrain_style(),
            Path::Terrain3d => snapshot.terrain_3d_style(),
            Path::Labels => snapshot.labels_style(),
        }
    }

    /// The camera of each of the path's [`FRAMES`] frames, by its index.
    pub fn camera(self, snapshot: &Snapshot) -> impl Fn(usize) -> CameraOptions + use<> {
        let corner = match self {
            Path::Overzoom => {
                let (x, y) = tile_at(13, LatLng::new(37.785, -122.4075).expect("Union Square"));
                tile_center(13, x, y)
            }
            Path::Street => {
                let (x, y) = tile_at(12, snapshot.area_center(Tiles::Street));
                tile_center(12, x, y)
            }
            Path::Globe | Path::Antimeridian | Path::Earthquakes | Path::Terrain | Path::Terrain3d | Path::Labels => {
                LatLng::default()
            }
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
                Path::Overzoom => CameraOptions {
                    center: Some(corner),
                    zoom: Some(22.0),
                    bearing: Some(360.0 * t),
                    pitch: Some(60.0),
                    ..Default::default()
                },
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
                Path::Earthquakes => {
                    let turn = std::f64::consts::TAU * t;
                    CameraOptions {
                        center: Some(LatLng::new(36.5 + 2.0 * turn.sin(), 138.0 + 3.0 * turn.cos()).expect("a center")),
                        zoom: Some(5.0 + 3.0 * (std::f64::consts::PI * t).sin()),
                        ..Default::default()
                    }
                }
                Path::Terrain | Path::Terrain3d => {
                    let turn = std::f64::consts::TAU * t;
                    let wave = (std::f64::consts::PI * t).sin();
                    CameraOptions {
                        center: Some(
                            LatLng::new(37.752 + 0.006 * turn.sin(), -122.447 + 0.008 * turn.cos()).expect("a center"),
                        ),
                        zoom: Some(13.0),
                        bearing: Some(90.0 * wave),
                        pitch: Some(30.0 + 15.0 * wave),
                        ..Default::default()
                    }
                }
                Path::Labels => CameraOptions {
                    // Union Square.
                    center: Some(LatLng::new(37.7880, -122.4075).expect("a center")),
                    zoom: Some(16.0),
                    bearing: Some(360.0 * t),
                    pitch: Some(60.0),
                    ..Default::default()
                },
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
        for path in Path::ALL {
            assert_eq!(Path::parse(path.name()), Ok(path));
        }
        assert!(Path::parse("moon").is_err());
    }
}
