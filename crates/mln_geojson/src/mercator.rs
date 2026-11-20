//! Web Mercator, scaled so that the world is the unit square, with y down.

use std::f64::consts::PI;

use geo_types::Coord;

/// The position of longitude `x`, latitude `y` (in degrees). Latitudes past
/// Web Mercator's limit (about ±85.05°) land on the square's top or bottom
/// edge.
#[must_use]
pub(crate) fn project(lng_lat: Coord) -> Coord {
    let sin = (lng_lat.y * PI / 180.0).sin();
    let y = 0.5 - 0.25 * ((1.0 + sin) / (1.0 - sin)).ln() / PI;
    Coord { x: lng_lat.x / 360.0 + 0.5, y: y.clamp(0.0, 1.0) }
}

/// The longitude and latitude (in degrees) of position `p`.
#[must_use]
pub(crate) fn unproject(p: Coord) -> Coord {
    let y = (180.0 - p.y * 360.0) * PI / 180.0;
    Coord { x: (p.x - 0.5) * 360.0, y: 360.0 * y.exp().atan() / PI - 90.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_the_world_onto_the_unit_square() {
        assert_eq!(project(Coord { x: 0.0, y: 0.0 }), Coord { x: 0.5, y: 0.5 });
        assert_eq!(project(Coord { x: -180.0, y: 90.0 }), Coord { x: 0.0, y: 0.0 });
        assert_eq!(project(Coord { x: 180.0, y: -90.0 }), Coord { x: 1.0, y: 1.0 });
    }

    #[test]
    fn unprojects_what_it_projects() {
        let lng_lat = Coord { x: -73.98, y: 40.75 };
        let back = unproject(project(lng_lat));
        assert!((back.x - lng_lat.x).abs() < 1e-9 && (back.y - lng_lat.y).abs() < 1e-9, "{back:?}");
    }
}
