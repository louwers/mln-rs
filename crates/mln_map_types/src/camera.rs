// Ports include/mln/map/camera.hpp (CameraOptions, AnimationOptions,
// FreeCameraOptions; the FreeCameraOptions methods are in util/camera.cpp,
// see util_camera.rs).

use mln_util::UnitBezier;
use mln_util::chrono::Duration;
use mln_util::geo::{EdgeInsets, LatLng, ScreenCoordinate};
use mln_util::vectors::{Vec3, Vec4};

/// Various options for describing the viewpoint of a map. All fields are
/// optional.
/// A partial command replaces animations of its specified fields. Omitted
/// fields keep their original animation timing. Flights couple center and
/// zoom; anchored commands couple center with their specified fields.
/// Replacing any member of a coupled group stops that group.
/// Anchor and center points are mutually exclusive, with preference for the
/// center point when both are set.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraOptions {
    /// Coordinate at the center of the map.
    pub center: Option<LatLng>,
    /// Altitude of the center of the map, in meters above sea level.
    pub center_altitude: Option<f64>,
    /// Padding around the interior of the view that affects the frame of
    /// reference for `center`.
    pub padding: Option<EdgeInsets>,
    /// Point of reference for `zoom` and `angle`, assuming an origin at the
    /// top-left corner of the view.
    pub anchor: Option<ScreenCoordinate>,
    /// Zero-based zoom level. Constrained to the minimum and maximum zoom
    /// levels.
    pub zoom: Option<f64>,
    /// Bearing, measured in degrees from true north. Wrapped to [0, 360).
    pub bearing: Option<f64>,
    /// Pitch toward the horizon measured in degrees, with 0 deg resulting in
    /// a two-dimensional map.
    pub pitch: Option<f64>,
    /// Roll, measured in degrees.
    pub roll: Option<f64>,
    /// Field of view, measured in degrees.
    pub fov: Option<f64>,
}

impl CameraOptions {
    pub fn with_center(mut self, o: Option<LatLng>) -> Self {
        self.center = o;
        self
    }
    pub fn with_center_altitude(mut self, o: Option<f64>) -> Self {
        self.center_altitude = o;
        self
    }
    pub fn with_padding(mut self, p: Option<EdgeInsets>) -> Self {
        self.padding = p;
        self
    }
    pub fn with_anchor(mut self, o: Option<ScreenCoordinate>) -> Self {
        self.anchor = o;
        self
    }
    pub fn with_zoom(mut self, o: Option<f64>) -> Self {
        self.zoom = o;
        self
    }
    pub fn with_bearing(mut self, o: Option<f64>) -> Self {
        self.bearing = o;
        self
    }
    pub fn with_pitch(mut self, o: Option<f64>) -> Self {
        self.pitch = o;
        self
    }
    pub fn with_roll(mut self, o: Option<f64>) -> Self {
        self.roll = o;
        self
    }
    pub fn with_fov(mut self, o: Option<f64>) -> Self {
        self.fov = o;
        self
    }
}

/// Various options for describing a transition between viewpoints with
/// animation. All fields are optional; the default values depend on how
/// this struct is used.
///
/// The callbacks are `Rc<dyn Fn>` so the struct can be copied into the
/// transition closures, as the C++ lambdas copy their `std::function`s.
#[derive(Clone, Default)]
pub struct AnimationOptions {
    /// Time to animate to the viewpoint defined herein.
    pub duration: Option<Duration>,
    /// Average velocity of a flyTo() transition, measured in screenfuls per
    /// second, assuming a linear timing curve.
    pub velocity: Option<f64>,
    /// Zero-based zoom level at the peak of the flyTo() transition's flight
    /// path.
    pub min_zoom: Option<f64>,
    /// The easing timing curve of the transition.
    pub easing: Option<UnitBezier>,
    /// A function that is called on each frame of the transition, just before
    /// a screen update, except on the last frame. The first parameter
    /// indicates the elapsed time as a percentage of the duration.
    pub transition_frame_fn: Option<std::rc::Rc<dyn Fn(f64)>>,
    /// Called once when all properties of the command have completed or been
    /// superseded, including explicit cancellation. Other commands may still
    /// be animating when this callback runs.
    pub transition_finish_fn: Option<std::rc::Rc<dyn Fn()>>,
    /// Caller-chosen identity of the command, which
    /// `Map::cancel_transitions_with_id` matches. Several commands may share
    /// one.
    pub transition_id: Option<u64>,
}

impl AnimationOptions {
    /// `AnimationOptions(Duration d)`.
    pub fn new(d: Duration) -> Self {
        AnimationOptions { duration: Some(d), ..Default::default() }
    }
}

/// Various options for accessing physical properties of the underlying camera
/// entity. A direct access to these properties allows more flexible and
/// precise controlling of the camera while also being fully compatible and
/// interchangeable with CameraOptions. All fields are optional.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FreeCameraOptions {
    /// Position of the camera in slightly modified web mercator coordinates.
    /// The size of 1 unit is the width of the projected world instead of the
    /// "mercator meter". Coordinate [0, 0, 0] is the north-west corner and
    /// [1, 1, 0] is the south-east corner. Z coordinate is conformal and must
    /// respect minimum and maximum zoom values. Zoom is automatically
    /// computed from the altitude (z).
    pub position: Option<Vec3>,
    /// Orientation of the camera represented as a unit quaternion [x, y, z,
    /// w].
    pub orientation: Option<Vec4>,
}
