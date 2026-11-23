//! The planes that the camera's moves solve on a Mercator view. The flat
//! map's ground is the plane at sea level: a move around an anchor, a drag,
//! a pinch, a turn or a tilt, holds the location its pointer grabbed on that
//! plane under the pointer. A move without an anchor, or around one whose
//! line of sight doesn't meet the ground, moves the map on the plane at the
//! center's altitude, which keeps the center where it draws. The globe holds
//! its locations on the sea-level sphere.

use mln_map_types::View;
use mln_util::error::Error;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};

use super::{Transform, flip_y};

/// How near the view's center, in squared pixels, an anchor stands at it:
/// a move around it leaves the center where it is (GL JS's).
const NEAR_CENTER: f64 = 1e-2;

impl Transform {
    /// The height of the plane on which a move around `anchor` (y down)
    /// holds the ground the pointer grabbed under `grabbed`, the pointer
    /// before the move: the flat map's ground, sea level.
    ///
    /// `None`, for the plane at the center's altitude, on the globe, for a
    /// pivot (`grabbed` is `anchor`) at the view's center, and where the
    /// plane can't hold the anchor or the grabbed point ([`sees_plane`]),
    /// such as one above the horizon.
    pub(super) fn anchor_plane(&self, grabbed: &ScreenCoordinate, anchor: &ScreenCoordinate) -> Option<f64> {
        let view = self.view();
        if view.is_globe() {
            return None;
        }
        let height = 0.0;
        let center = view.padding().center_in(view.size());
        // A pivot at the center leaves the center where it is.
        let pivot = grabbed == anchor && (anchor.x - center.x).powi(2) + (anchor.y - center.y).powi(2) < NEAR_CENTER;
        let sees = |point: &ScreenCoordinate| sees_plane(view, &flip_y(view, point), height);
        (!pivot && sees(anchor) && sees(grabbed)).then_some(height)
    }

    /// The height of the plane on which a move around `anchor` (y down)
    /// holds the location under it ([`Self::anchor_plane`] with `anchor` as
    /// what it grabbed), or else the plane at the center's altitude. On the
    /// globe, sea level, the sphere the globe holds its locations on.
    pub(super) fn anchor_height(&self, anchor: &ScreenCoordinate) -> f64 {
        if self.view().is_globe() {
            return 0.0;
        }
        self.anchor_plane(anchor, anchor).unwrap_or(self.view().center_altitude())
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
/// sea level under `point` of a Mercator `view`, measured from the bottom
/// edge (y up): whether the point's line of sight starts above the plane and
/// descends to meet it before the far plane, which the view fits to the
/// horizon. It asks the line itself, which a roll turns away from the
/// screen's rows. Never on the globe, nor while the view shows nothing.
pub(super) fn sees_plane(view: &View, point: &ScreenCoordinate, height: f64) -> bool {
    let Some((near, far)) = view.line_of_sight(point) else { return false };
    far.z < near.z && far.z <= height && height < near.z
}
