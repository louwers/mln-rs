//! The projections that the example apps register with their runtimes.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use mln_style::crs::{Crs, CrsConverter, TileMatrixSet};

/// EPSG:3413, NSIDC Sea Ice Polar Stereographic North, which the catalog's
/// Arctic style draws in (examples/styles.json): the polar stereographic
/// projection of the WGS 84 ellipsoid, true to scale at 70° N, with 45° W
/// pointing down from the North Pole, on the tile grid that NASA GIBS serves
/// it in: tile 0/0/0 is the square 8,388,608 m on a side centered on the
/// North Pole.
#[must_use]
pub fn epsg3413() -> Crs {
    Crs {
        name: "EPSG:3413".to_owned(),
        converter: Box::new(PolarStereographic::new()),
        tile_matrix_set: TileMatrixSet { origin: [-4_194_304.0, 4_194_304.0], extent_at_zoom0: 8_388_608.0 },
    }
}

/// The WGS 84 ellipsoid: its semi-major axis in meters, and its flattening.
const SEMI_MAJOR_AXIS: f64 = 6_378_137.0;
const FLATTENING: f64 = 1.0 / 298.257_223_563;

/// The latitude of true scale, and the longitude that points down from the
/// pole, in degrees.
const TRUE_SCALE_LATITUDE: f64 = 70.0;
const CENTRAL_MERIDIAN: f64 = -45.0;

/// The north polar aspect of the ellipsoidal polar stereographic projection,
/// by the formulas of Snyder's Map Projections: A Working Manual (1987):
/// forward by (15-9), (14-15), (21-34), (21-30) and (21-31), inverse by
/// (20-18), (21-40), (7-13), the series (3-5) and (20-16). The South Pole,
/// and latitudes past either pole, have no place on the plane.
struct PolarStereographic {
    /// The ellipsoid's eccentricity.
    eccentricity: f64,
    /// The distance from the pole in meters per unit of Snyder's t.
    rho_per_t: f64,
    /// The coefficients of sin 2χ, sin 4χ, sin 6χ and sin 8χ in the series
    /// that takes the conformal latitude χ to the latitude.
    series: [f64; 4],
}

impl PolarStereographic {
    fn new() -> PolarStereographic {
        let e2 = FLATTENING * (2.0 - FLATTENING);
        let eccentricity = e2.sqrt();
        let latitude = TRUE_SCALE_LATITUDE.to_radians();
        let m = latitude.cos() / (1.0 - e2 * latitude.sin().powi(2)).sqrt();
        let (e4, e6, e8) = (e2.powi(2), e2.powi(3), e2.powi(4));
        PolarStereographic {
            eccentricity,
            rho_per_t: SEMI_MAJOR_AXIS * m / t(eccentricity, latitude),
            series: [
                e2 / 2.0 + 5.0 * e4 / 24.0 + e6 / 12.0 + 13.0 * e8 / 360.0,
                7.0 * e4 / 48.0 + 29.0 * e6 / 240.0 + 811.0 * e8 / 11_520.0,
                7.0 * e6 / 120.0 + 81.0 * e8 / 1_120.0,
                4_279.0 * e8 / 161_280.0,
            ],
        }
    }
}

/// Snyder's t of a latitude in radians on an ellipsoid of eccentricity `e`:
/// 0 at the North Pole, growing toward the South Pole.
fn t(e: f64, latitude: f64) -> f64 {
    let e_sin = e * latitude.sin();
    (FRAC_PI_4 - latitude / 2.0).tan() / ((1.0 - e_sin) / (1.0 + e_sin)).powf(e / 2.0)
}

impl CrsConverter for PolarStereographic {
    fn forward(&self, lng_lats: &[[f64; 2]], positions: &mut [[f64; 2]]) {
        for (&[longitude, latitude], position) in lng_lats.iter().zip(positions) {
            *position = if latitude > -90.0 && latitude <= 90.0 {
                let rho = self.rho_per_t * t(self.eccentricity, latitude.to_radians());
                let (sin, cos) = (longitude - CENTRAL_MERIDIAN).to_radians().sin_cos();
                [rho * sin, -rho * cos]
            } else {
                [f64::NAN; 2]
            };
        }
    }

    /// Longitudes come out at least -180° and under 180°.
    fn inverse(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        let [a, b, c, d] = self.series;
        for (&[x, y], lng_lat) in positions.iter().zip(lng_lats) {
            let chi = FRAC_PI_2 - 2.0 * (x.hypot(y) / self.rho_per_t).atan();
            let latitude =
                chi + a * (2.0 * chi).sin() + b * (4.0 * chi).sin() + c * (6.0 * chi).sin() + d * (8.0 * chi).sin();
            let longitude = CENTRAL_MERIDIAN + x.atan2(-y).to_degrees();
            *lng_lat = [(longitude + 180.0).rem_euclid(360.0) - 180.0, latitude.to_degrees()];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Longitude and latitude in degrees, and x and y in meters, as PROJ
    /// 9.9.0 converts them between EPSG:4326 and EPSG:3413 with `cs2cs`.
    const PROJ: [([f64; 2], [f64; 2]); 17] = [
        // From lng/lat: the North Pole, the central meridian at the latitude
        // of true scale, the Arctic style's center, Nuuk, Longyearbyen, the
        // meridian opposite the central one, then out to the southern
        // hemisphere.
        ([0.0, 90.0], [0.0, 0.0]),
        ([-45.0, 70.0], [0.0, -2_187_927.649_279_021]),
        ([-40.0, 78.0], [113_695.402_766_497, -1_299_544.400_204_582]),
        ([-51.72, 64.18], [-332_729.077_184_709, -2_823_880.458_853_276]),
        ([15.63, 78.22], [1_115_855.845_659_671, -627_982.919_287_297]),
        ([135.0, 60.0], [0.0, 3_323_160.270_641_002]),
        ([45.0, 45.0], [5_131_675.567_848_917, 0.0]),
        ([-120.0, 30.0], [-6_899_449.069_545_792, -1_848_701.806_404_458]),
        ([-90.0, 0.0], [-8_718_901.806_961_387, -8_718_901.806_961_39]),
        ([100.0, -45.0], [16_993_611.228_176_09, 24_269_392.002_870_087]),
        // From x/y: the corners and the middle of the bottom edge of tile
        // 0/0/0, and two places inside it.
        ([-180.0, 38.807_151_293_313], [-4_194_304.0, 4_194_304.0]),
        ([90.0, 38.807_151_293_313], [4_194_304.0, 4_194_304.0]),
        ([-90.0, 38.807_151_293_313], [-4_194_304.0, -4_194_304.0]),
        ([0.0, 38.807_151_293_313], [4_194_304.0, -4_194_304.0]),
        ([-45.0, 52.613_210_675_815], [0.0, -4_194_304.0]),
        ([108.434_948_822_922, 69.568_765_756_559], [1_000_000.0, 2_000_000.0]),
        ([-104.036_243_467_926, 63.547_536_372_521], [-2_500_000.0, -1_500_000.0]),
    ];

    fn forward(lng_lat: [f64; 2]) -> [f64; 2] {
        let mut positions = [[0.0; 2]];
        epsg3413().converter.forward(&[lng_lat], &mut positions);
        positions[0]
    }

    fn inverse(position: [f64; 2]) -> [f64; 2] {
        let mut lng_lats = [[0.0; 2]];
        epsg3413().converter.inverse(&[position], &mut lng_lats);
        lng_lats[0]
    }

    /// How close a position must come, in meters, and a place, in degrees,
    /// about as far on the ground: Snyder's series for the latitude is good
    /// to a tenth of a millimeter over tile 0/0/0 and as far again around it.
    const METERS: f64 = 1e-4;
    const DEGREES: f64 = 1e-9;

    fn meters_apart(a: [f64; 2], b: [f64; 2]) -> f64 {
        (a[0] - b[0]).hypot(a[1] - b[1])
    }

    /// How far apart two places are on the ground, in degrees of latitude:
    /// a degree of longitude counts for less away from the equator, and for
    /// nothing at the poles, where every longitude is the same place.
    fn degrees_apart(a: [f64; 2], b: [f64; 2]) -> f64 {
        let longitude = (a[0] - b[0] + 180.0).rem_euclid(360.0) - 180.0;
        (longitude * a[1].to_radians().cos()).hypot(a[1] - b[1])
    }

    #[test]
    fn converts_as_proj_does() {
        for (lng_lat, position) in PROJ {
            let forward = forward(lng_lat);
            assert!(meters_apart(forward, position) < METERS, "{lng_lat:?} goes to {forward:?}, PROJ's {position:?}");
            let inverse = inverse(position);
            assert!(degrees_apart(inverse, lng_lat) < DEGREES, "{position:?} goes to {inverse:?}, PROJ's {lng_lat:?}");
        }
    }

    #[test]
    fn round_trips() {
        // Every 5° of latitude from 85° S to the North Pole, every 15° of
        // longitude.
        for latitude in (-17..=18).map(|step| f64::from(step) * 5.0) {
            for longitude in (-12..12).map(|step| f64::from(step) * 15.0) {
                let back = inverse(forward([longitude, latitude]));
                assert!(
                    degrees_apart(back, [longitude, latitude]) < DEGREES,
                    "{longitude}, {latitude} comes back {back:?}"
                );
            }
        }
        // Every 500 km over tile 0/0/0 and as far again around it.
        for x in (-16..=16).map(|step| f64::from(step) * 500_000.0) {
            for y in (-16..=16).map(|step| f64::from(step) * 500_000.0) {
                let back = forward(inverse([x, y]));
                assert!(meters_apart(back, [x, y]) < METERS, "{x}, {y} comes back {back:?}");
            }
        }
    }

    #[test]
    fn has_no_place_for_the_south_pole_and_beyond() {
        for lng_lat in [[0.0, -90.0], [0.0, -90.5], [0.0, 90.5], [10.0, f64::NAN], [f64::NAN, 45.0]] {
            let position = forward(lng_lat);
            assert!(position.iter().all(|value| value.is_nan()), "{lng_lat:?} goes to {position:?}");
        }
        assert!(inverse([f64::NAN, 0.0]).iter().all(|value| value.is_nan()));
    }

    #[test]
    fn the_arctic_style_draws_in_it() {
        let catalog: serde_json::Value = serde_json::from_str(include_str!("../../styles.json")).unwrap();
        let styles = catalog["styles"].as_array().unwrap();
        let arctic = styles.iter().find(|style| style["key"] == "arctic").unwrap();
        assert_eq!(arctic["json"]["projection"]["type"], epsg3413().name.as_str());
    }
}
