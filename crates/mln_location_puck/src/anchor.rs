//! Where a geographic position is in a view: its anchors, one in each world
//! copy that what draws around it can reach the view from, and where a
//! point around one shows on the screen.

use geometry::Point;
use mln_engine::camera::{TileCoordinate, TileProjector, View};
use mln_engine::geo::ScreenCoordinate;
use mln_engine::geo::constants::{EARTH_RADIUS_M, M2PI};
use mln_engine::tile::UnwrappedTileID;
use mln_engine::tile_store::TileLodMode;
use mln_engine::tile_store::tile_cover::{TileCoverParameters, tile_cover};
use mln_style::constants::{EXTENT, LATITUDE_MAX, TILE_SIZE};
use mln_style::geo::{LatLng, WrapMode};
use mln_style::range::Range;
use mln_style::tile_id::CanonicalTileID;

/// Where a geographic position is in a view, in one world copy: in the tile
/// at the view's integer zoom that holds it there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Anchor {
    /// The tile, in the world copy.
    pub(crate) tile: UnwrappedTileID,
    /// The position, in the tile's units from its top left corner.
    pub(crate) point: [f64; 2],
    /// The height of the ground under the position, in meters.
    pub(crate) height: f64,
    /// The tile units that a logical pixel spans at the view's zoom.
    pub(crate) pixels_to_tile_units: f64,
    /// The tile units that a meter on the ground spans at the position.
    pub(crate) tile_units_per_meter: f64,
    /// How far the position is from the camera, in pixels, as
    /// [`View::camera_to_center_distance`] is the center's.
    pub(crate) distance: f64,
}

/// The anchors of `at` in `view`, standing on the ground whose height, in
/// meters, `height` gives at a point of a tile: on Mercator, one in each
/// world copy the view shows and in the copy past each end, from which
/// what draws around the position can reach into the view; on the globe,
/// one, unless the globe hides the position where it draws, at its height
/// and through the projection as blended. None behind the camera.
pub(crate) fn anchors(view: &View, at: LatLng, height: impl Fn(UnwrappedTileID, [f64; 2]) -> f64) -> Vec<Anchor> {
    let zoom = view.integer_zoom();
    let tiles = f64::from(1u32 << zoom);
    let at = LatLng::saturating(at.latitude().clamp(-LATITUDE_MAX, LATITUDE_MAX), at.longitude(), WrapMode::Unwrapped);
    let tile_units_per_meter = tiles * f64::from(EXTENT) / (M2PI * EARTH_RADIUS_M * at.latitude().to_radians().cos());
    let world = |at: &LatLng| TileCoordinate::from_lat_lng(0.0, at).p;

    // The world copies' wraps, each with where the position is in it, in
    // worlds from its top left.
    let mut copies: Vec<(i16, Point<f64>)> = Vec::new();
    if view.is_globe() {
        let p = world(&view.lat_lng_near_center(&at));
        copies.push((p.x.floor() as i16, Point::new(p.x - p.x.floor(), p.y)));
    } else {
        let p = world(&at.wrapped());
        let wraps = world_copies(view);
        if let (Some(first), Some(last)) = (wraps.iter().min(), wraps.iter().max()) {
            copies.extend((first - 1..=last + 1).map(|wrap| (wrap, p)));
        }
    }

    let pixels_to_tile_units = f64::from(EXTENT) / (TILE_SIZE * 2f64.powf(view.zoom() - f64::from(zoom)));
    let extent = f64::from(EXTENT);
    let last = tiles as u32 - 1;
    copies
        .into_iter()
        .filter_map(|(wrap, p)| {
            let (x, y) = (p.x * tiles, p.y * tiles);
            let canonical = CanonicalTileID::new(zoom, (x as u32).min(last), (y.max(0.0) as u32).min(last));
            let tile = UnwrappedTileID::with_wrap(wrap, canonical);
            let point = [(x - f64::from(canonical.x)) * extent, (y - f64::from(canonical.y)) * extent];
            let height = height(tile, point);
            let drawn = TileProjector::new(view, &tile).project_as_drawn(&Point::new(point[0], point[1]), height);
            let distance = drawn.signed_distance_from_camera;
            (distance > 0.0 && !drawn.occluded).then_some(Anchor {
                tile,
                point,
                height,
                pixels_to_tile_units,
                tile_units_per_meter,
                distance,
            })
        })
        .collect()
}

/// The wraps of the world copies whose zoom-0 tiles cover `view`.
fn world_copies(view: &View) -> Vec<i16> {
    let parameters = TileCoverParameters {
        view: view.clone(),
        tile_lod_min_radius: 3.0,
        tile_lod_scale: 1.0,
        tile_lod_pitch_threshold: std::f64::consts::FRAC_PI_3,
        tile_lod_mode: TileLodMode::Default,
        requested_zoom: None,
        round_zoom: false,
        max_content_elevation: 0.0,
    };
    tile_cover(&parameters, 0, Range::new(0, 0), None).iter().map(|tile| tile.wrap).collect()
}

/// `point`, in the units of `tile`, `height` meters up, where it draws on
/// `view`'s screen, through the projection as blended, in logical pixels
/// from its top left; `None` where the globe hides it or it is behind the
/// camera.
pub(crate) fn to_screen(view: &View, tile: UnwrappedTileID, point: [f64; 2], height: f64) -> Option<ScreenCoordinate> {
    let at = TileProjector::new(view, &tile).project_as_drawn(&Point::new(point[0], point[1]), height);
    if at.occluded || at.signed_distance_from_camera <= 0.0 {
        return None;
    }
    let size = view.size();
    // Clip space's y goes up.
    Some(ScreenCoordinate::new(
        (at.point.x + 1.0) * f64::from(size.width) * 0.5,
        (1.0 - at.point.y) * f64::from(size.height) * 0.5,
    ))
}

#[cfg(test)]
mod tests {
    use mln_engine::camera::TransformState;
    use mln_style::size::Size;
    use mln_style::types::{ProjectionDefinition, ProjectionType};

    use super::*;

    fn camera(latitude: f64, longitude: f64, zoom: f64, globe: bool) -> TransformState {
        let mut state = TransformState::default();
        state.set_size(Size::new(512, 256));
        if globe {
            state.set_projection_definition(&ProjectionDefinition::new(ProjectionType::Globe));
        }
        state.set_lat_lng_zoom(&LatLng::new(latitude, longitude).unwrap(), zoom);
        state
    }

    fn flat(view: &View, at: LatLng) -> Vec<Anchor> {
        anchors(view, at, |_, _| 0.0)
    }

    /// Where `view` shows `at`, from the top left of its screen.
    fn screen(view: &View, at: LatLng) -> ScreenCoordinate {
        let point = view.lat_lng_to_screen_coordinate(&at);
        ScreenCoordinate::new(point.x, f64::from(view.size().height) - point.y)
    }

    fn assert_near(expected: ScreenCoordinate, actual: ScreenCoordinate) {
        assert!(
            (expected.x - actual.x).abs() < 1e-6 && (expected.y - actual.y).abs() < 1e-6,
            "{expected:?} {actual:?}"
        );
    }

    #[test]
    fn an_anchor_just_west_of_the_antimeridian_lies_in_a_tile_of_the_zoom() {
        let state = camera(0.0, 179.0, 3.0, false);
        // Each projects to the world's east edge.
        for longitude in [180f64.next_down(), -540.0] {
            let at = LatLng::saturating(10.0, longitude, WrapMode::Unwrapped);
            let anchors = flat(state.view(), at);
            assert!(!anchors.is_empty(), "{longitude}");
            for anchor in anchors {
                assert_eq!(7, anchor.tile.canonical.x, "{longitude}");
                assert_eq!(f64::from(EXTENT), anchor.point[0], "{longitude}");
            }
        }
    }

    #[test]
    fn an_anchor_is_the_position_in_a_tile_of_the_views_integer_zoom() {
        let state = camera(10.0, 20.0, 3.5, false);
        let view = state.view();
        let anchors = flat(view, LatLng::new(12.0, 21.0).unwrap());
        // The view shows one world copy, between the copies past its ends.
        assert_eq!(vec![-1, 0, 1], anchors.iter().map(|anchor| anchor.tile.wrap).collect::<Vec<_>>());
        for anchor in &anchors {
            assert_eq!(CanonicalTileID::new(3, 4, 3), anchor.tile.canonical);
            assert!(anchor.point.iter().all(|v| (0.0..f64::from(EXTENT)).contains(v)), "{anchor:?}");
            let copy = LatLng::new(12.0, 21.0 + 360.0 * f64::from(anchor.tile.wrap)).unwrap();
            assert_near(screen(view, copy), to_screen(view, anchor.tile, anchor.point, 0.0).unwrap());
        }
        let expected = 8.0 * f64::from(EXTENT) / (M2PI * EARTH_RADIUS_M * 12f64.to_radians().cos());
        assert!((anchors[1].tile_units_per_meter - expected).abs() < 1e-9);
        assert!((anchors[1].distance - view.camera_to_center_distance()).abs() < 1.0);
        let tile_units = f64::from(anchors[1].tile.pixels_to_tile_units(1.0, view.zoom() as f32));
        assert!((anchors[1].pixels_to_tile_units - tile_units).abs() < 1e-4);
    }

    #[test]
    fn the_copy_across_the_antimeridian_anchors_beside_the_center() {
        for globe in [false, true] {
            let state = camera(0.0, 179.0, 5.0, globe);
            let view = state.view();
            let beside = flat(view, LatLng::new(0.0, -179.0).unwrap())
                .into_iter()
                .filter_map(|anchor| to_screen(view, anchor.tile, anchor.point, 0.0))
                .filter(|point| (point.x - 256.0).abs() < 256.0)
                .count();
            assert_eq!(1, beside, "globe: {globe}");
        }
    }

    #[test]
    fn the_globe_anchors_a_position_once_where_it_shows() {
        let state = camera(0.0, 0.0, 1.0, true);
        let view = state.view();
        let anchors = flat(view, LatLng::new(30.0, 40.0).unwrap());
        assert_eq!(1, anchors.len());
        assert_near(
            screen(view, LatLng::new(30.0, 40.0).unwrap()),
            to_screen(view, anchors[0].tile, anchors[0].point, 0.0).unwrap(),
        );
        assert_eq!(flat(view, LatLng::new(0.0, 180.0).unwrap()), []);
    }
}
