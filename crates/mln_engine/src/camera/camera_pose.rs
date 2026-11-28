//! The camera as a position and an orientation in Web Mercator space, the
//! form that free camera options set and that the projection matrix is built
//! from.

use std::f64::consts::PI;

use glam::{DMat4, DQuat, DVec3};
use mln_style::geo::LatLng;

use crate::camera::camera::FreeCameraOptions;
use crate::geo::LatLngAltitude;
use crate::geo::constants::M2PI;
use crate::geo::projection::Projection;

fn mercator_x_from_lng(lng: f64) -> f64 {
    (180.0 + lng) / 360.0
}

fn mercator_y_from_lat(lat: f64) -> f64 {
    (180.0 - (180.0 / PI * ((PI / 4.0) + lat * PI / 360.0).tan().ln())) / 360.0
}

fn lat_from_mercator_y(y: f64) -> f64 {
    mln_style::math::rad2deg(2.0 * (PI - y * M2PI).exp().atan() - (PI / 2.0))
}

fn lng_from_mercator_x(x: f64) -> f64 {
    x * 360.0 - 180.0
}

/// The free camera position, in Web Mercator units, of `location` at
/// `altitude_meters` above it.
pub(crate) fn to_mercator(location: &LatLng, altitude_meters: f64) -> DVec3 {
    let pixels_per_meter = 1.0 / Projection::meters_per_pixel_at_latitude(location.latitude(), 0.0);
    let world_size = Projection::world_size(1.0);
    DVec3::new(
        mercator_x_from_lng(location.longitude()),
        mercator_y_from_lat(location.latitude()),
        altitude_meters * pixels_per_meter / world_size,
    )
}

/// The lat/lng and the altitude in meters that a free camera's Web Mercator
/// `position` stands for, at any position whose lat/lng isn't NaN.
pub(crate) fn lat_lng_altitude_from_mercator(position: DVec3) -> Option<LatLngAltitude> {
    let location = LatLng::new(lat_from_mercator_y(position.y), lng_from_mercator_x(position.x)).ok()?;
    let meters_per_pixel = Projection::meters_per_pixel_at_latitude(location.latitude(), 0.0);
    let altitude = position.z * Projection::world_size(1.0) * meters_per_pixel;
    Some(LatLngAltitude { location, altitude })
}

/// The rotation of a camera with these angles, in radians: it turns `bearing`
/// clockwise about the up axis, tilts `pitch` toward the horizon, then rolls.
pub(crate) fn orientation_from_roll_pitch_bearing(roll: f64, pitch: f64, bearing: f64) -> DQuat {
    DQuat::from_axis_angle(DVec3::Z, -bearing)
        * DQuat::from_axis_angle(DVec3::X, -pitch)
        * DQuat::from_axis_angle(DVec3::Z, roll)
}

/// The roll, pitch and bearing of a camera that looks along `forward` with
/// `right` and `up` at its sides, in radians. A camera looking straight down
/// has no roll: its bearing takes it.
fn roll_pitch_bearing(forward: DVec3, right: DVec3, up: DVec3) -> (f64, f64, f64) {
    let f01 = forward.truncate().length();
    let pitch = f01.atan2(-forward.z);
    let (bearing, roll) = if f01 > 1.0e-3 {
        (-forward.x.atan2(-forward.y), -right.z.atan2(up.z))
    } else {
        ((-right.y).atan2(right.x), 0.0)
    };
    (roll, pitch, bearing)
}

/// A camera's position, in Web Mercator units where the world is 1 wide
/// (z included), and its orientation. It looks along -z toward the map, with
/// y toward the south.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CameraPose {
    pub(crate) position: DVec3,
    pub(crate) orientation: DQuat,
}

impl Default for CameraPose {
    fn default() -> Self {
        CameraPose { position: DVec3::ZERO, orientation: DQuat::IDENTITY }
    }
}

impl CameraPose {
    /// A pose at `position` with the orientation of these angles, in radians.
    pub(crate) fn from_angles(position: DVec3, roll: f64, pitch: f64, bearing: f64) -> Self {
        CameraPose { position, orientation: orientation_from_roll_pitch_bearing(roll, pitch, bearing) }
    }

    /// The matrix from world space, in pixels at `scale` with z in meters of
    /// `pixels_per_meter` pixels, to camera space, with y down unless
    /// `flipped_y`.
    pub(crate) fn world_to_camera(&self, scale: f64, pixels_per_meter: f64, flipped_y: bool) -> DMat4 {
        let world_size = Projection::world_size(scale);
        let flip = if flipped_y { DMat4::IDENTITY } else { DMat4::from_scale(DVec3::new(1.0, -1.0, 1.0)) };
        flip * DMat4::from_quat(self.orientation.conjugate())
            * DMat4::from_translation(-self.position * world_size)
            * DMat4::from_scale(DVec3::new(1.0, 1.0, pixels_per_meter))
    }

    /// The direction the camera looks, toward the map.
    pub(crate) fn forward(&self) -> DVec3 {
        self.orientation * DVec3::NEG_Z
    }

    pub(crate) fn right(&self) -> DVec3 {
        self.orientation * DVec3::X
    }

    /// The camera's up direction. Mercator's y points south, so it is the
    /// rotated -y.
    pub(crate) fn up(&self) -> DVec3 {
        self.orientation * DVec3::NEG_Y
    }

    /// The camera's orientation as `(roll, pitch, bearing)`, in radians.
    pub(crate) fn orientation_angles(&self) -> (f64, f64, f64) {
        roll_pitch_bearing(self.forward(), self.right(), self.up())
    }

    /// The orientation of a camera looking along `forward` with `up` above
    /// it, without the twist that a free orientation can have, or `None` if
    /// the two are parallel.
    pub(crate) fn orientation_from_frame(forward: DVec3, up: DVec3) -> Option<DQuat> {
        let right = up.cross(forward);
        if right.length() < 1e-15 {
            return None;
        }
        let (roll, pitch, bearing) = roll_pitch_bearing(forward, right, up);
        Some(orientation_from_roll_pitch_bearing(roll, pitch, bearing))
    }
}

impl FreeCameraOptions {
    /// Helper function for setting the mercator position as Lat&Lng and
    /// altitude in meters.
    #[cfg(test)]
    pub(crate) fn set_location(&mut self, location: &LatLngAltitude) {
        self.position = Some(to_mercator(&location.location, location.altitude));
    }

    /// Helper function for converting mercator position into Lat&Lng and
    /// altitude in meters. This function fails to return a value if
    /// `position` is invalid or is not set.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn location(&self) -> Option<LatLngAltitude> {
        let position_value = self.position?;
        if !(0.0..=1.0).contains(&position_value.y) {
            return None;
        }
        lat_lng_altitude_from_mercator(position_value)
    }

    /// Helper function for setting orientation of the camera by defining a
    /// focus point on the map. Up vector is required in certain scenarios
    /// where bearing can't be deduced from the viewing direction.
    #[cfg(test)]
    pub(crate) fn look_at_point(&mut self, location: &LatLng, up_vector: Option<DVec3>) {
        self.orientation = None;
        let Some(position) = self.position else {
            return;
        };

        let forward = to_mercator(location, 0.0) - position;
        let mut up = up_vector.unwrap_or(DVec3::Z);

        // Flip z-component of the up vector if it's pointing downwards
        up.z = up.z.abs();

        self.orientation = CameraPose::orientation_from_frame(forward, up);
    }

    /// Helper function for setting the orientation of the camera as a roll,
    /// pitch, and a bearing. All values are in degrees.
    #[cfg(test)]
    pub(crate) fn set_roll_pitch_bearing(&mut self, roll: f64, pitch: f64, bearing: f64) {
        self.orientation = Some(orientation_from_roll_pitch_bearing(
            mln_style::math::deg2rad(roll),
            mln_style::math::deg2rad(pitch),
            mln_style::math::deg2rad(bearing),
        ));
    }
}

#[cfg(test)]
#[path = "camera_pose_test.rs"]
mod camera_pose_test;
