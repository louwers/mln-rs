//! EPSG:27700, the British National Grid, from WGS84 through the Helmert
//! transformation, on Ordnance Survey's tile grid: the projection that the
//! `projection/national-grid` render tests draw in.
//!
//! The formulas and constants are those of Ordnance Survey's "A Guide to
//! Coordinate Systems in Great Britain", V3.6 (2020): annex A for the
//! constants, B for Cartesian coordinates, C for the transverse Mercator
//! projection, section 6.6 for the Helmert transformation.

use mln_style::crs::{Crs, CrsConverter, TileMatrixSet};

const RADIANS_PER_DEGREE: f64 = std::f64::consts::PI / 180.0;
const RADIANS_PER_ARC_SECOND: f64 = RADIANS_PER_DEGREE / 3600.0;

/// An ellipsoid by its semi-major and semi-minor axes, in meters.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ellipsoid {
    a: f64,
    b: f64,
}

impl Ellipsoid {
    /// The square of the eccentricity.
    fn e2(self) -> f64 {
        (self.a * self.a - self.b * self.b) / (self.a * self.a)
    }
}

pub(crate) const AIRY_1830: Ellipsoid = Ellipsoid { a: 6_377_563.396, b: 6_356_256.909 };
pub(crate) const GRS80: Ellipsoid = Ellipsoid { a: 6_378_137.0, b: 6_356_752.314_1 };

/// A latitude and a longitude in degrees, and a height above the ellipsoid in
/// meters.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LatLngHeight {
    pub(crate) latitude: f64,
    pub(crate) longitude: f64,
    pub(crate) height: f64,
}

/// `position` on `ellipsoid` to Cartesian coordinates (B2 to B5).
pub(crate) fn cartesian_from_lat_lng_height(ellipsoid: Ellipsoid, position: LatLngHeight) -> [f64; 3] {
    let phi = position.latitude * RADIANS_PER_DEGREE;
    let lambda = position.longitude * RADIANS_PER_DEGREE;
    let e2 = ellipsoid.e2();
    let nu = ellipsoid.a / (1.0 - e2 * phi.sin() * phi.sin()).sqrt();
    [
        (nu + position.height) * phi.cos() * lambda.cos(),
        (nu + position.height) * phi.cos() * lambda.sin(),
        ((1.0 - e2) * nu + position.height) * phi.sin(),
    ]
}

/// Cartesian coordinates to a latitude, a longitude and a height on
/// `ellipsoid` (B6 to B9), iterating until the latitude moves by less than a
/// micrometer.
pub(crate) fn lat_lng_height_from_cartesian(ellipsoid: Ellipsoid, [x, y, z]: [f64; 3]) -> LatLngHeight {
    let latitude_tolerance = 1e-6 / 6_378_137.0;
    let e2 = ellipsoid.e2();
    let p = x.hypot(y);
    let nu_at = |phi: f64| ellipsoid.a / (1.0 - e2 * phi.sin() * phi.sin()).sqrt();
    let mut phi = (z / (p * (1.0 - e2))).atan();
    loop {
        let next = ((z + e2 * nu_at(phi) * phi.sin()) / p).atan();
        let change = next - phi;
        phi = next;
        if change.abs() < latitude_tolerance {
            break;
        }
    }
    LatLngHeight {
        latitude: phi / RADIANS_PER_DEGREE,
        longitude: y.atan2(x) / RADIANS_PER_DEGREE,
        height: p / phi.cos() - nu_at(phi),
    }
}

/// A seven-parameter Helmert transformation of Cartesian coordinates
/// (section 6.2, equation 3).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Helmert {
    translation: [f64; 3],
    scale_ppm: f64,
    rotation_arc_seconds: [f64; 3],
}

impl Helmert {
    pub(crate) fn apply(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let s = 1.0 + self.scale_ppm * 1e-6;
        let [rx, ry, rz] = self.rotation_arc_seconds.map(|r| r * RADIANS_PER_ARC_SECOND);
        let [tx, ty, tz] = self.translation;
        [tx + s * x - rz * y + ry * z, ty + rz * x + s * y - rx * z, tz - ry * x + rx * y + s * z]
    }

    /// The reverse transformation, as section 6.2 gives it: every parameter
    /// negated.
    fn reversed(&self) -> Helmert {
        Helmert {
            translation: self.translation.map(|t| -t),
            scale_ppm: -self.scale_ppm,
            rotation_arc_seconds: self.rotation_arc_seconds.map(|r| -r),
        }
    }
}

/// WGS84 or ETRS89, which differ negligibly here, to OSGB36 (section 6.6,
/// table 4). It is up to 3.5 m (95%) from the National Grid's definitive
/// transformation, OSTN15.
pub(crate) const ETRS89_TO_OSGB36: Helmert = Helmert {
    translation: [-446.448, 125.157, -542.060],
    scale_ppm: 20.4894,
    rotation_arc_seconds: [-0.1502, -0.2470, -0.8421],
};

/// A transverse Mercator projection of an ellipsoid (annex C).
#[derive(Clone, Copy, Debug)]
pub(crate) struct TransverseMercator {
    ellipsoid: Ellipsoid,
    scale: f64,
    origin_latitude: f64,
    central_longitude: f64,
    false_easting: f64,
    false_northing: f64,
}

/// The radii of curvature in the prime vertical and along the meridian,
/// scaled by F0, and their ratio less one (C2).
struct Radii {
    nu: f64,
    rho: f64,
    eta2: f64,
}

impl TransverseMercator {
    /// A latitude and a longitude in degrees to an easting and a northing in
    /// meters (C1 to C5).
    pub(crate) fn forward(&self, latitude: f64, longitude: f64) -> [f64; 2] {
        let phi = latitude * RADIANS_PER_DEGREE;
        let (sin_phi, cos_phi) = phi.sin_cos();
        let tan2 = phi.tan() * phi.tan();
        let Radii { nu, rho, eta2 } = self.radii(phi);
        let i = self.meridian_arc(phi) + self.false_northing;
        let ii = nu / 2.0 * sin_phi * cos_phi;
        let iii = nu / 24.0 * sin_phi * cos_phi.powf(3.0) * (5.0 - tan2 + 9.0 * eta2);
        let iiia = nu / 720.0 * sin_phi * cos_phi.powf(5.0) * (61.0 - 58.0 * tan2 + tan2 * tan2);
        let iv = nu * cos_phi;
        let v = nu / 6.0 * cos_phi.powf(3.0) * (nu / rho - tan2);
        let vi = nu / 120.0 * cos_phi.powf(5.0) * (5.0 - 18.0 * tan2 + tan2 * tan2 + 14.0 * eta2 - 58.0 * tan2 * eta2);
        let d_lambda = (longitude - self.central_longitude) * RADIANS_PER_DEGREE;
        [
            self.false_easting + iv * d_lambda + v * d_lambda.powf(3.0) + vi * d_lambda.powf(5.0),
            i + ii * d_lambda * d_lambda + iii * d_lambda.powf(4.0) + iiia * d_lambda.powf(6.0),
        ]
    }

    /// An easting and a northing in meters to a latitude and a longitude in
    /// degrees (C6 to C9), iterating until the northing of the latitude found
    /// is within 0.01 mm.
    pub(crate) fn inverse(&self, easting: f64, northing: f64) -> [f64; 2] {
        let northing_tolerance = 0.01e-3;
        let a_f0 = self.ellipsoid.a * self.scale;
        let mut phi = (northing - self.false_northing) / a_f0 + self.origin_latitude * RADIANS_PER_DEGREE;
        let mut residual = northing - self.false_northing - self.meridian_arc(phi);
        while residual.abs() >= northing_tolerance {
            phi += residual / a_f0;
            residual = northing - self.false_northing - self.meridian_arc(phi);
        }
        let tan_phi = phi.tan();
        let tan2 = tan_phi * tan_phi;
        let sec_phi = 1.0 / phi.cos();
        let Radii { nu, rho, eta2 } = self.radii(phi);
        let vii = tan_phi / (2.0 * rho * nu);
        let viii = tan_phi / (24.0 * rho * nu.powf(3.0)) * (5.0 + 3.0 * tan2 + eta2 - 9.0 * tan2 * eta2);
        let ix = tan_phi / (720.0 * rho * nu.powf(5.0)) * (61.0 + 90.0 * tan2 + 45.0 * tan2 * tan2);
        let x = sec_phi / nu;
        let xi = sec_phi / (6.0 * nu.powf(3.0)) * (nu / rho + 2.0 * tan2);
        let xii = sec_phi / (120.0 * nu.powf(5.0)) * (5.0 + 28.0 * tan2 + 24.0 * tan2 * tan2);
        let xiia =
            sec_phi / (5040.0 * nu.powf(7.0)) * (61.0 + 662.0 * tan2 + 1320.0 * tan2 * tan2 + 720.0 * tan2.powf(3.0));
        let d_e = easting - self.false_easting;
        let latitude = phi - vii * d_e * d_e + viii * d_e.powf(4.0) - ix * d_e.powf(6.0);
        let d_lambda = x * d_e - xi * d_e.powf(3.0) + xii * d_e.powf(5.0) - xiia * d_e.powf(7.0);
        [latitude / RADIANS_PER_DEGREE, self.central_longitude + d_lambda / RADIANS_PER_DEGREE]
    }

    fn radii(&self, phi: f64) -> Radii {
        let e2 = self.ellipsoid.e2();
        let w = 1.0 - e2 * phi.sin() * phi.sin();
        let nu = self.ellipsoid.a * self.scale / w.sqrt();
        let rho = self.ellipsoid.a * self.scale * (1.0 - e2) / w.powf(1.5);
        Radii { nu, rho, eta2: nu / rho - 1.0 }
    }

    /// The meridional arc from the true origin's latitude to `phi`, scaled by
    /// F0 (C3).
    fn meridian_arc(&self, phi: f64) -> f64 {
        let Ellipsoid { a, b } = self.ellipsoid;
        let n = (a - b) / (a + b);
        let (n2, n3) = (n * n, n * n * n);
        let phi0 = self.origin_latitude * RADIANS_PER_DEGREE;
        let (d, s) = (phi - phi0, phi + phi0);
        b * self.scale
            * ((1.0 + n + 5.0 * n2 / 4.0 + 5.0 * n3 / 4.0) * d
                - (3.0 * n + 3.0 * n2 + 21.0 * n3 / 8.0) * d.sin() * s.cos()
                + (15.0 * n2 / 8.0 + 15.0 * n3 / 8.0) * (2.0 * d).sin() * (2.0 * s).cos()
                - (35.0 * n3 / 24.0) * (3.0 * d).sin() * (3.0 * s).cos())
    }
}

/// The National Grid's projection of OSGB36 (annex A.2).
pub(crate) const NATIONAL_GRID_PROJECTION: TransverseMercator = TransverseMercator {
    ellipsoid: AIRY_1830,
    scale: 0.999_601_271_7,
    origin_latitude: 49.0,
    central_longitude: -2.0,
    false_easting: 400_000.0,
    false_northing: -100_000.0,
};

/// A WGS84 latitude and longitude at sea level to a National Grid easting
/// and northing.
pub(crate) fn easting_northing_from_wgs84(latitude: f64, longitude: f64) -> [f64; 2] {
    let wgs84 = cartesian_from_lat_lng_height(GRS80, LatLngHeight { latitude, longitude, height: 0.0 });
    let osgb36 = lat_lng_height_from_cartesian(AIRY_1830, ETRS89_TO_OSGB36.apply(wgs84));
    NATIONAL_GRID_PROJECTION.forward(osgb36.latitude, osgb36.longitude)
}

/// The guide's way back from a National Grid easting and northing to WGS84:
/// the projection's inverse series, then the Helmert transformation
/// reversed. Over Britain it lands a few millimeters from the point that
/// [`easting_northing_from_wgs84`] takes there, since the reversed
/// transformation isn't quite the inverse and the height it leaves out isn't
/// zero.
fn approximate_wgs84_from_easting_northing(easting: f64, northing: f64) -> [f64; 2] {
    let [latitude, longitude] = NATIONAL_GRID_PROJECTION.inverse(easting, northing);
    let osgb36 = cartesian_from_lat_lng_height(AIRY_1830, LatLngHeight { latitude, longitude, height: 0.0 });
    let wgs84 = lat_lng_height_from_cartesian(GRS80, ETRS89_TO_OSGB36.reversed().apply(osgb36));
    [wgs84.latitude, wgs84.longitude]
}

/// A National Grid easting and northing to the WGS84 latitude and longitude
/// that [`easting_northing_from_wgs84`] takes back to it within a
/// micrometer. Each step moves the target of the guide's way back by what is
/// left.
pub(crate) fn wgs84_from_easting_northing(easting: f64, northing: f64) -> [f64; 2] {
    let tolerance = 1e-6;
    let max_steps = 8;
    let mut target = [easting, northing];
    let mut lat_lng = approximate_wgs84_from_easting_northing(easting, northing);
    for _ in 0..max_steps {
        let [reached_easting, reached_northing] = easting_northing_from_wgs84(lat_lng[0], lat_lng[1]);
        let (easting_left, northing_left) = (easting - reached_easting, northing - reached_northing);
        if easting_left.hypot(northing_left) < tolerance {
            break;
        }
        target = [target[0] + easting_left, target[1] + northing_left];
        lat_lng = approximate_wgs84_from_easting_northing(target[0], target[1]);
    }
    lat_lng
}

/// The side of an Ordnance Survey level 0 tile in meters. Level 0 is 5 by 6
/// of them from E -238375, N 1376256.
const LEVEL_0_TILE_SIZE: f64 = 229_376.0;

/// The tile zoom of OS level 0: the registered tile 0/0/0 is the 8 by 8
/// square of level 0 tiles around Great Britain.
const LEVEL_0_ZOOM: i32 = 3;

/// The converter of [`national_grid`].
struct NationalGrid;

impl CrsConverter for NationalGrid {
    fn forward(&self, lng_lats: &[[f64; 2]], positions: &mut [[f64; 2]]) {
        for (position, [longitude, latitude]) in positions.iter_mut().zip(lng_lats) {
            *position = easting_northing_from_wgs84(*latitude, *longitude);
        }
    }

    fn inverse(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        for (lng_lat, [easting, northing]) in lng_lats.iter_mut().zip(positions) {
            let [latitude, longitude] = wgs84_from_easting_northing(*easting, *northing);
            *lng_lat = [longitude, latitude];
        }
    }
}

/// EPSG:27700, the British National Grid, under `name`: WGS84 through the
/// Helmert transformation, on Ordnance Survey's tile grid, where a tile's
/// zoom is its OS level plus 3.
pub(crate) fn national_grid(name: &str) -> Crs {
    Crs {
        name: name.to_owned(),
        converter: Box::new(NationalGrid),
        tile_matrix_set: TileMatrixSet {
            origin: [-238_375.0, 1_376_256.0],
            extent_at_zoom0: LEVEL_0_TILE_SIZE * f64::from(1 << LEVEL_0_ZOOM),
        },
    }
}

#[cfg(test)]
mod tests;
