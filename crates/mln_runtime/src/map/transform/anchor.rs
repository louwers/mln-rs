//! The planes that the camera's moves solve on, or on the globe the
//! spheres. The flat map's ground is the plane, or the sphere, at sea level:
//! a move around an anchor, a drag, a pinch, a turn or a tilt, holds the
//! location its pointer grabbed on it under the pointer. A move without an
//! anchor, or around one whose line of sight doesn't meet the ground, moves
//! the map on the plane or the sphere at the center's altitude, which keeps
//! the center where it draws.

use mln_map_types::{VerticalPerspectiveProjection, View};
use mln_util::error::Error;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};

use super::{Transform, flip_y};

/// How near the view's center, in squared pixels, an anchor stands at it:
/// a move around it leaves the center where it is (GL JS's).
const NEAR_CENTER: f64 = 1e-2;

impl Transform {
    /// The height of the plane on which a move around `anchor` (y down)
    /// holds the ground the pointer grabbed under `grabbed`, the pointer
    /// before the move: the flat map's ground, sea level. On the globe the
    /// plane is the sphere at that height.
    ///
    /// `None`, for the plane at the center's altitude, for a pivot
    /// (`grabbed` is `anchor`) at the view's center, and where the plane
    /// can't hold the anchor or the grabbed point ([`sees_plane`]), such as
    /// one above the horizon.
    pub(super) fn anchor_plane(&self, grabbed: &ScreenCoordinate, anchor: &ScreenCoordinate) -> Option<f64> {
        let view = self.view();
        let height = 0.0;
        let center = view.padding().center_in(view.size());
        // A pivot at the center leaves the center where it is.
        let pivot = grabbed == anchor && (anchor.x - center.x).powi(2) + (anchor.y - center.y).powi(2) < NEAR_CENTER;
        let sees = |point: &ScreenCoordinate| sees_plane(view, &flip_y(view, point), height);
        (!pivot && sees(anchor) && sees(grabbed)).then_some(height)
    }

    /// The height of the plane, or on the globe the sphere, on which a move
    /// around `anchor` (y down) holds the location the pointer grabbed under
    /// `grabbed` ([`Self::anchor_plane`]), or else the center's altitude.
    pub(super) fn anchor_height(&self, grabbed: &ScreenCoordinate, anchor: &ScreenCoordinate) -> f64 {
        self.anchor_plane(grabbed, anchor).unwrap_or(self.view().center_altitude())
    }

    /// The center that moves the location on the plane `height` meters
    /// above sea level under `from` to `to`, both screen points (y down),
    /// unwrapped.
    pub(super) fn center_moving_on_plane(
        &self,
        from: &ScreenCoordinate,
        to: &ScreenCoordinate,
        height: f64,
    ) -> Result<LatLng, Error> {
        let location = self.screen_coordinate_to_lat_lng_at(from, height, WrapMode::Unwrapped)?;
        self.state.center_for_lat_lng_at(&location, &flip_y(self.view(), to), height)
    }
}

/// Whether a move can hold a location on the plane `height` meters above
/// sea level under `point` of `view`, measured from the bottom edge (y up):
/// whether the point's line of sight starts above the plane and descends to
/// meet it before the far plane, which the view fits to the horizon. It asks
/// the line itself, which a roll turns away from the screen's rows. On the
/// globe the plane is the sphere at that height, which the eye must be
/// above and the line must meet. Never while the view shows nothing.
pub(super) fn sees_plane(view: &View, point: &ScreenCoordinate, height: f64) -> bool {
    if view.is_globe() {
        let above =
            view.globe_camera_position().length() > 1.0 + height / VerticalPerspectiveProjection::GLOBE_RADIUS_METERS;
        return above && VerticalPerspectiveProjection::screen_coordinate_hits_globe(view, point, height);
    }
    let Some((near, far)) = view.line_of_sight(point) else { return false };
    far.z < near.z && far.z <= height && height < near.z
}
