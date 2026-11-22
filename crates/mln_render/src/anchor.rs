//! Where a geographic position is in a frame, for a layer that draws what it
//! makes itself rather than its tiles' data: its [`Anchor`]s, and where
//! geometry around one draws ([`Frame::tile`]) and shows on the screen
//! ([`FrameQuery::to_screen`]).

use geometry::Point;
use mln_map_types::tile_coordinate::TileCoordinate;
use mln_map_types::tile_cover::{TileCoverParameters, tile_cover};
use mln_map_types::{TileProjector, View};
use mln_tile_id::{CanonicalTileID, UnwrappedTileID};
use mln_util::constants::{EARTH_RADIUS_M, EXTENT, LATITUDE_MAX, M2PI, TILE_SIZE};
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::range::Range;

use crate::layer::{FrameQuery, Layer};
use crate::paint_parameters::Frame;
use crate::tile_matrix::{TileProjection, TileTransform, projection_data};

/// Where a geographic position is in a frame, in one world copy: in the tile
/// at the camera's integer zoom that holds it there. Geometry around the
/// position, in the tile's units, draws as [`Frame::tile`] projects the tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    /// The tile, in the world copy.
    pub tile: UnwrappedTileID,
    /// The position, in the tile's units from its top left corner.
    pub point: [f64; 2],
    /// The tile units that a logical pixel spans at the camera's zoom.
    pub pixels_to_tile_units: f64,
    /// The tile units that a meter on the ground spans at the position.
    pub tile_units_per_meter: f64,
    /// How far the position is from the camera, in pixels, as
    /// [`View::camera_to_center_distance`] is the center's.
    pub distance: f64,
}

impl Frame<'_> {
    /// The anchors of `at` in the frame: one in each world copy that the
    /// frame shows, and in the copy past each end, from which what a layer
    /// draws around the position can reach into the view; on the globe, one.
    /// None where the globe hides the position or it is behind the camera.
    pub fn anchors(&self, at: LatLng) -> impl Iterator<Item = Anchor> + use<> {
        anchors(self.camera, at)
    }

    /// Where geometry in the units of `tile` draws in the frame, placed as
    /// `how` says: of any tile, such as an [`Anchor`]'s. A draw pushed with
    /// it doesn't clip.
    #[must_use]
    pub fn tile(&self, tile: UnwrappedTileID, how: &TileTransform) -> TileProjection {
        TileProjection::of(&tile, self.camera, &projection_data(&tile, self, how), None)
    }
}

impl<L: Layer> FrameQuery<'_, L> {
    /// The anchors of `at` in the last frame, as [`Frame::anchors`] gives
    /// them.
    pub fn anchors(&self, at: LatLng) -> impl Iterator<Item = Anchor> + use<L> {
        anchors(self.camera, at)
    }

    /// `point`, in the units of `tile`, on the screen, in the camera's
    /// pixels from its top left as the query's geometry is: where geometry
    /// around an [`Anchor`] shows. `None` where the globe hides it or it is
    /// behind the camera.
    #[must_use]
    pub fn to_screen(&self, tile: UnwrappedTileID, point: [f64; 2]) -> Option<ScreenCoordinate> {
        to_screen(self.camera, tile, point)
    }
}

/// `point` of `tile` on `camera`'s screen (see [`FrameQuery::to_screen`]).
fn to_screen(camera: &View, tile: UnwrappedTileID, point: [f64; 2]) -> Option<ScreenCoordinate> {
    let at = TileProjector::new(camera, &tile).project(&Point::new(point[0], point[1]), 0.0);
    if at.occluded || at.signed_distance_from_camera <= 0.0 {
        return None;
    }
    let size = camera.size();
    // Clip space's y goes up.
    Some(ScreenCoordinate::new(
        (at.point.x + 1.0) * f64::from(size.width) * 0.5,
        (1.0 - at.point.y) * f64::from(size.height) * 0.5,
    ))
}

/// The anchors of `at` that `camera` shows (see [`Frame::anchors`]).
fn anchors(camera: &View, at: LatLng) -> impl Iterator<Item = Anchor> + use<> {
    let zoom = camera.integer_zoom();
    let tiles = f64::from(1u32 << zoom);
    let at = LatLng::saturating(at.latitude().clamp(-LATITUDE_MAX, LATITUDE_MAX), at.longitude(), WrapMode::Unwrapped);
    let tile_units_per_meter = tiles * f64::from(EXTENT) / (M2PI * EARTH_RADIUS_M * at.latitude().to_radians().cos());
    let world = |at: &LatLng| TileCoordinate::from_lat_lng(0.0, at).p;

    // The world copies' wraps, each with where the position is in it, in
    // worlds from its top left.
    let mut copies: Vec<(i16, Point<f64>)> = Vec::new();
    if camera.is_globe() {
        if !camera.is_location_occluded(&at) {
            let p = world(&camera.lat_lng_near_center(&at));
            copies.push((p.x.floor() as i16, Point::new(p.x - p.x.floor(), p.y)));
        }
    } else {
        let p = world(&at.wrapped());
        let cover = tile_cover(&TileCoverParameters::new(camera.clone()), 0, Range::new(0, 0), None);
        if let (Some(first), Some(last)) = (cover.iter().map(|t| t.wrap).min(), cover.iter().map(|t| t.wrap).max()) {
            copies.extend((first - 1..=last + 1).map(|wrap| (wrap, p)));
        }
    }

    let pixels_to_tile_units = f64::from(EXTENT) / (TILE_SIZE * 2f64.powf(camera.zoom() - f64::from(zoom)));
    let anchors = copies.into_iter().filter_map(move |(wrap, p)| {
        let (x, y) = (p.x * tiles, p.y * tiles);
        let canonical = CanonicalTileID::new(zoom, x as u32, (y.max(0.0) as u32).min((tiles as u32) - 1));
        let tile = UnwrappedTileID::with_wrap(wrap, canonical);
        let extent = f64::from(EXTENT);
        let point = [(x - f64::from(canonical.x)) * extent, (y - f64::from(canonical.y)) * extent];
        let distance =
            TileProjector::new(camera, &tile).project(&Point::new(point[0], point[1]), 0.0).signed_distance_from_camera;
        (distance > 0.0).then_some(Anchor { tile, point, pixels_to_tile_units, tile_units_per_meter, distance })
    });
    anchors.collect::<Vec<_>>().into_iter()
}

#[cfg(test)]
mod tests {
    use mln_map_types::TransformState;
    use mln_style_types::{ProjectionDefinition, ProjectionType};
    use mln_util::size::Size;

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

    /// Where `camera` shows `at`, from the top left of its screen.
    fn screen(camera: &View, at: LatLng) -> ScreenCoordinate {
        let point = camera.lat_lng_to_screen_coordinate(&at);
        ScreenCoordinate::new(point.x, f64::from(camera.size().height) - point.y)
    }

    fn assert_near(expected: ScreenCoordinate, actual: ScreenCoordinate) {
        assert!(
            (expected.x - actual.x).abs() < 1e-6 && (expected.y - actual.y).abs() < 1e-6,
            "{expected:?} {actual:?}"
        );
    }

    #[test]
    fn an_anchor_is_the_position_in_a_tile_of_the_cameras_integer_zoom() {
        let state = camera(10.0, 20.0, 3.5, false);
        let view = state.view();
        let at = LatLng::new(12.0, 21.0).unwrap();
        let anchors: Vec<Anchor> = anchors(view, at).collect();
        // The view shows one world copy, between the copies past its ends.
        assert_eq!(vec![-1, 0, 1], anchors.iter().map(|anchor| anchor.tile.wrap).collect::<Vec<_>>());
        for anchor in &anchors {
            assert_eq!(CanonicalTileID::new(3, 4, 3), anchor.tile.canonical);
            assert!(anchor.point.iter().all(|v| (0.0..f64::from(EXTENT)).contains(v)), "{anchor:?}");
            let copy = LatLng::new(12.0, 21.0 + 360.0 * f64::from(anchor.tile.wrap)).unwrap();
            assert_near(screen(view, copy), to_screen(view, anchor.tile, anchor.point).unwrap());
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
            let beside = anchors(view, LatLng::new(0.0, -179.0).unwrap())
                .filter_map(|anchor| to_screen(view, anchor.tile, anchor.point))
                .filter(|point| (point.x - 256.0).abs() < 256.0)
                .count();
            assert_eq!(1, beside, "globe: {globe}");
        }
    }

    #[test]
    fn the_globe_anchors_a_position_once_where_it_shows() {
        let state = camera(0.0, 0.0, 1.0, true);
        let view = state.view();
        let anchors: Vec<Anchor> = anchors(view, LatLng::new(30.0, 40.0).unwrap()).collect();
        assert_eq!(1, anchors.len());
        assert_near(
            screen(view, LatLng::new(30.0, 40.0).unwrap()),
            to_screen(view, anchors[0].tile, anchors[0].point).unwrap(),
        );
        assert_eq!(0, super::anchors(view, LatLng::new(0.0, 180.0).unwrap()).count());
    }
}
