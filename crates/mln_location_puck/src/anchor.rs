//! Where a geographic position is in a view: its anchors, one in each world
//! copy that what draws around it can reach the view from, and where a
//! point around one shows on the screen.

use geometry::Point;
use glam::DVec3;
use mln_engine::camera::{TileCoordinate, TileProjector, VerticalPerspectiveProjection, View};
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
    /// The tile units that a logical pixel spans at the position: at the
    /// view's zoom and, on the globe, at the center's latitude, as the
    /// globe scales tile units away from it.
    pub(crate) pixels_to_tile_units: f64,
    /// The tile units that a meter on the ground spans at the position: in
    /// a registered projection, as many as the view's meters per pixel
    /// there make it ([`View::meters_per_pixel_at`]).
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
/// and through the projection as blended; in a registered projection, one,
/// where its world square places the position, unless it has no place
/// there. None behind the camera.
pub(crate) fn anchors(view: &View, at: LatLng, height: impl Fn(UnwrappedTileID, [f64; 2]) -> f64) -> Vec<Anchor> {
    let zoom = view.integer_zoom();
    let tiles = f64::from(1u32 << zoom);
    let pixels_to_tile_units = f64::from(EXTENT) / (TILE_SIZE * 2f64.powf(view.zoom() - f64::from(zoom)));

    // The world copies' wraps, each with where the position is in it, in
    // worlds from its top left, and the tile units a meter spans there.
    let mut copies: Vec<(i16, Point<f64>)> = Vec::new();
    let tile_units_per_meter = if view.world().wraps() {
        let at =
            LatLng::saturating(at.latitude().clamp(-LATITUDE_MAX, LATITUDE_MAX), at.longitude(), WrapMode::Unwrapped);
        let world = |at: &LatLng| TileCoordinate::from_lat_lng(0.0, at).p;
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
        tiles * f64::from(EXTENT) / (M2PI * EARTH_RADIUS_M * at.latitude().to_radians().cos())
    } else {
        // A registered projection's world doesn't repeat, and its scale
        // varies across it in ways only its converter knows.
        let p = view.world().world_from_lat_lng(&at);
        if p.x.is_finite() && p.y.is_finite() {
            copies.push((0, p));
        }
        pixels_to_tile_units / view.meters_per_pixel_at(&at)
    };

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
            let projector = TileProjector::new(view, &tile);
            let drawn = projector.project_as_drawn(&Point::new(point[0], point[1]), height);
            let distance = drawn.signed_distance_from_camera;
            (distance > 0.0 && !drawn.occluded).then_some(Anchor {
                tile,
                point,
                height,
                pixels_to_tile_units: pixels_to_tile_units
                    * projector.pitched_text_correction(&Point::new(point[0], point[1])),
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

/// How much of what stands at `anchor` the ground in front of it leaves,
/// as the shaders' `ground_visibility` fades it: 1 where no ground nearer
/// the camera shows at the anchor's screen point, or where that point is
/// outside the view, down to 0 where the nearest ground there is in front
/// of the anchor by GL JS's ramp of depth. `height` gives the ground's
/// height, in meters, at a point of the anchor's tile or past its edges.
/// The line of sight is sampled every 4 pixels across the ground, at most
/// 512 times.
pub(crate) fn ground_visibility(
    view: &View,
    anchor: &Anchor,
    height: impl Fn(UnwrappedTileID, [f64; 2]) -> f64,
) -> f64 {
    let Some(screen) = to_screen(view, anchor.tile, anchor.point, anchor.height) else {
        return 1.0;
    };
    let size = view.size();
    let (width, view_height) = (f64::from(size.width), f64::from(size.height));
    if !(0.0..=width).contains(&screen.x) || !(0.0..=view_height).contains(&screen.y) {
        return 1.0;
    }

    // The line of sight, as a line from the anchor to the eye, or to where
    // the near plane cuts it, through Mercator's tile units and meters, or
    // through the globe's unit-sphere space; and its point at `t`, from 0 at
    // the anchor to 1, as a point of the tile and a height.
    let tile = anchor.tile;
    let (tiles, extent) = (f64::from(1u32 << tile.canonical.z), f64::from(EXTENT));
    let to_tile = |x: f64, y: f64| {
        [
            ((x - f64::from(tile.wrap)) * tiles - f64::from(tile.canonical.x)) * extent,
            (y * tiles - f64::from(tile.canonical.y)) * extent,
        ]
    };
    let anchor_x = (f64::from(tile.canonical.x) + anchor.point[0] / extent) / tiles + f64::from(tile.wrap);
    let globe = view.is_globe();
    let (start, end) = if globe {
        let surface = VerticalPerspectiveProjection::tile_coordinates_to_sphere(
            &Point::new(anchor.point[0], anchor.point[1]),
            &tile,
        );
        (
            surface * (1.0 + anchor.height / VerticalPerspectiveProjection::GLOBE_RADIUS_METERS),
            view.globe_camera_position(),
        )
    } else {
        let Some((near, _)) = view.line_of_sight(&ScreenCoordinate::new(screen.x, view_height - screen.y)) else {
            return 1.0;
        };
        let [x, y] = to_tile(near.x, near.y);
        (DVec3::new(anchor.point[0], anchor.point[1], anchor.height), DVec3::new(x, y, near.z))
    };
    let at = |t: f64| {
        let q = start.lerp(end, t);
        if !globe {
            return ([q.x, q.y], q.z);
        }
        let (radius, n) = (q.length(), q.normalize());
        let location = LatLng::saturating(n.y.asin().to_degrees(), n.x.atan2(n.z).to_degrees(), WrapMode::Unwrapped);
        let p = TileCoordinate::from_lat_lng(0.0, &location).p;
        // In the world copy of the anchor.
        let x = p.x + (anchor_x - p.x).round();
        (to_tile(x, p.y), (radius - 1.0) * VerticalPerspectiveProjection::GLOBE_RADIUS_METERS)
    };
    let under = |t: f64| {
        let (point, ray) = at(t);
        height(tile, point) > ray
    };

    let (eye, _) = at(1.0);
    let across = (eye[0] - anchor.point[0]).hypot(eye[1] - anchor.point[1]) / anchor.pixels_to_tile_units;
    let steps = (across / 4.0).ceil().clamp(1.0, 512.0) as u32;
    let Some(step) = (1..=steps).find(|&i| under(1.0 - f64::from(i) / f64::from(steps))) else {
        return 1.0;
    };
    let (mut above, mut below) =
        (1.0 - f64::from(step - 1) / f64::from(steps), 1.0 - f64::from(step) / f64::from(steps));
    for _ in 0..24 {
        let mid = f64::midpoint(above, below);
        if under(mid) { below = mid } else { above = mid }
    }
    let (point, ray) = at(below);
    let ground = TileProjector::new(view, &tile).project_as_drawn(&Point::new(point[0], point[1]), ray);

    // GL JS's `depthOpacity`, with its ramp's scale.
    let depth_factor = view_height / 25.0;
    let d = depth_factor * (1.0 / anchor.distance - 1.0 / ground.signed_distance_from_camera) + 0.0001;
    1.0 - (-500.0 * d).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mln_engine::camera::TransformState;
    use mln_engine::geo::world::WorldMapping;
    use mln_style::crs::{Crs, CrsMapping};
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

    /// A camera on a map in the identity projection, whose world square
    /// spans longitudes from -90 to 90 and every latitude.
    fn identity_camera(latitude: f64, longitude: f64, zoom: f64) -> TransformState {
        let mut state = TransformState::default();
        state.set_size(Size::new(512, 256));
        state.set_world(WorldMapping::Crs(Arc::new(CrsMapping::new(Arc::new(Crs::identity())))));
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

    #[test]
    fn on_the_globe_a_logical_pixel_at_an_anchor_spans_one_on_the_screen_at_any_latitude() {
        let state = camera(0.0, 0.0, 3.0, true);
        let view = state.view();
        for latitude in [0.0, 30.0, 50.0] {
            let anchors = flat(view, LatLng::new(latitude, 0.0).unwrap());
            assert_eq!(1, anchors.len(), "{latitude}");
            let anchor = anchors[0];
            // East, across the line of sight, the screen shrinks a pixel by
            // how much farther it is than the center.
            let pixels = 10.0 * anchor.distance / view.camera_to_center_distance();
            let east = [anchor.point[0] + pixels * anchor.pixels_to_tile_units, anchor.point[1]];
            let from = to_screen(view, anchor.tile, anchor.point, 0.0).unwrap();
            let to = to_screen(view, anchor.tile, east, 0.0).unwrap();
            let length = (to.x - from.x).hypot(to.y - from.y);
            assert!((length - 10.0).abs() < 0.1, "{latitude}: {length}");
        }
    }

    #[test]
    fn in_a_registered_projection_a_position_anchors_once_where_the_projection_places_it() {
        let state = identity_camera(20.0, 10.0, 2.0);
        let view = state.view();
        // 112.5 and 67.5 degrees from the world square's west and north
        // edges: the middle of tile 2/2/1.
        let at = LatLng::new(22.5, 22.5).unwrap();
        let anchors = flat(view, at);
        assert_eq!(1, anchors.len());
        assert_eq!(UnwrappedTileID::with_wrap(0, CanonicalTileID::new(2, 2, 1)), anchors[0].tile);
        assert_eq!([4096.0, 4096.0], anchors[0].point);
        assert_near(screen(view, at), to_screen(view, anchors[0].tile, anchors[0].point, 0.0).unwrap());

        // Past Mercator's latitude limit, which the projection doesn't have.
        let north = LatLng::new(88.0, 22.5).unwrap();
        let anchors = flat(view, north);
        assert_eq!(1, anchors.len());
        assert_near(screen(view, north), to_screen(view, anchors[0].tile, anchors[0].point, 0.0).unwrap());
    }

    /// A meter spans a pixel's tile units over the map's meters per pixel at
    /// the position: here GL JS's `LngLat.distanceTo` across 100 pixels, over
    /// 100, where at zoom 2 the identity's 180 degrees are 2048 pixels and a
    /// pixel spans 16 tile units.
    #[test]
    fn in_a_registered_projection_a_meter_spans_what_the_maps_meters_per_pixel_make_it() {
        let state = identity_camera(20.0, 10.0, 2.0);
        let anchors = flat(state.view(), LatLng::new(20.0, 10.0).unwrap());
        assert_eq!(1, anchors.len());
        let meters_per_pixel = 16.0 / anchors[0].tile_units_per_meter;
        assert!((meters_per_pixel / 9_182.565_332_549_524 - 1.0).abs() < 1e-9, "{meters_per_pixel}");
    }
}
