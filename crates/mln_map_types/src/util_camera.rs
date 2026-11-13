// It lives here rather than in mln_util because camera.cpp also implements
// `FreeCameraOptions` (map/camera.hpp).

use std::f64::consts::PI;

use mln_util::constants::{EARTH_RADIUS_M, M2PI};
#[cfg(test)]
use mln_util::geo::{LatLng, LatLngAltitude};
#[cfg(test)]
use mln_util::mat3::vec3_sub;
use mln_util::mat3::{vec3_cross, vec3_length};
use mln_util::mat4::{Mat4, matrix};
use mln_util::projection::Projection;
use mln_util::quaternion::Quaternion;
use mln_util::vectors::Vec3;

use crate::camera::FreeCameraOptions;

#[cfg(test)]
fn mercator_x_from_lng(lng: f64) -> f64 {
    (180.0 + lng) / 360.0
}

#[cfg(test)]
fn mercator_y_from_lat(lat: f64) -> f64 {
    (180.0 - (180.0 / PI * ((PI / 4.0) + lat * PI / 360.0).tan().ln())) / 360.0
}

fn lat_from_mercator_y(y: f64) -> f64 {
    mln_math::rad2deg(2.0 * (PI - y * M2PI).exp().atan() - (PI / 2.0))
}

#[cfg(test)]
fn lng_from_mercator_x(x: f64) -> f64 {
    x * 360.0 - 180.0
}

#[cfg(test)]
fn to_mercator(location: &LatLng, altitude_meters: f64) -> Vec3 {
    let pixels_per_meter = 1.0 / Projection::get_meters_per_pixel_at_latitude(location.latitude(), 0.0);
    let world_size = Projection::world_size(2.0f64.powf(0.0));

    [
        mercator_x_from_lng(location.longitude()),
        mercator_y_from_lat(location.latitude()),
        altitude_meters * pixels_per_meter / world_size,
    ]
}

fn get_column(matrix: &Mat4, col: usize) -> [f64; 4] {
    debug_assert!(col < 4);
    [matrix[col * 4], matrix[col * 4 + 1], matrix[col * 4 + 2], matrix[col * 4 + 3]]
}

pub(crate) fn orientation_from_roll_pitch_bearing(roll: f64, pitch: f64, bearing: f64) -> Quaternion {
    // Both angles have to be negated to achieve CW rotation around the axis
    // of rotation
    let rot_bearing = Quaternion::from_axis_angle(&[0.0, 0.0, 1.0], -bearing);
    let rot_pitch = Quaternion::from_axis_angle(&[1.0, 0.0, 0.0], -pitch);
    let rot_roll = Quaternion::from_axis_angle(&[0.0, 0.0, 1.0], roll);

    rot_bearing.multiply(&rot_pitch).multiply(&rot_roll)
}

fn update_transform_orientation(transform: &mut Mat4, orientation: &Quaternion) {
    // Construct rotation matrix from orientation
    let mut m = orientation.to_rotation_matrix();

    // Apply translation to the matrix
    let translation = get_column(transform, 3);
    m[12] = translation[0];
    m[13] = translation[1];
    m[14] = translation[2];

    *transform = m;
}

fn update_transform_position(transform: &mut Mat4, position: &Vec3) {
    transform[12] = position[0];
    transform[13] = position[1];
    transform[14] = position[2];
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    orientation: Quaternion,
    /// Position and orientation of the camera.
    transform: Mat4,
}

impl Default for Camera {
    fn default() -> Self {
        Camera::new()
    }
}

impl Camera {
    #[must_use]
    pub fn new() -> Self {
        let mut transform = [0.0; 16];
        matrix::identity(&mut transform);
        Camera { orientation: Quaternion::IDENTITY, transform }
    }

    #[must_use]
    pub(crate) fn get_position(&self) -> Vec3 {
        let p = get_column(&self.transform, 3);
        [p[0], p[1], p[2]]
    }

    #[must_use]
    pub(crate) fn get_world_to_camera(&self, scale: f64, flipped_y: bool) -> Mat4 {
        // transformation chain from world space to camera space:
        // 1. Height value (z) of renderables is in meters. Scale z coordinate
        //    by pixelsPerMeter
        // 2. Transform from pixel coordinates to camera space with
        //    cameraMatrix^-1
        // 3. flip Y if required

        // worldToCamera: flip * cam^-1 * zScale
        // cameraToWorld: (flip * cam^-1 * zScale)^-1 => (zScale^-1 * cam * flip^-1)
        let world_size = Projection::world_size(scale);
        let latitude = lat_from_mercator_y(get_column(&self.transform, 3)[1]);
        let pixels_per_meter = world_size / (mln_math::deg2rad(latitude).cos() * M2PI * EARTH_RADIUS_M);

        // Compute inverse of the camera matrix
        let mut result = self.orientation.conjugate().to_rotation_matrix();

        let a = result;
        matrix::translate(
            &mut result,
            &a,
            -self.transform[12] * world_size,
            -self.transform[13] * world_size,
            -self.transform[14] * world_size,
        );

        if !flipped_y {
            // Pre-multiply y
            result[1] *= -1.0;
            result[5] *= -1.0;
            result[9] *= -1.0;
            result[13] *= -1.0;
        }

        // Post-multiply z
        result[8] *= pixels_per_meter;
        result[9] *= pixels_per_meter;
        result[10] *= pixels_per_meter;
        result[11] *= pixels_per_meter;

        result
    }

    #[must_use]
    #[expect(clippy::unused_self, reason = "a camera's projection, beside its other matrices")]
    pub(crate) fn get_camera_to_clip_perspective(&self, fovy: f64, aspect_ratio: f64, near_z: f64, far_z: f64) -> Mat4 {
        let mut projection = [0.0; 16];
        matrix::perspective(&mut projection, fovy, aspect_ratio, near_z, far_z);
        projection
    }

    #[must_use]
    pub(crate) fn forward(&self) -> Vec3 {
        let column = get_column(&self.transform, 2);
        // The forward direction is towards the map, [0, 0, -1]
        [-column[0], -column[1], -column[2]]
    }

    #[must_use]
    pub(crate) fn right(&self) -> Vec3 {
        let column = get_column(&self.transform, 0);
        [column[0], column[1], column[2]]
    }

    #[must_use]
    pub(crate) fn up(&self) -> Vec3 {
        let column = get_column(&self.transform, 1);
        // Up direction has to be flipped due to y-axis pointing towards south
        [-column[0], -column[1], -column[2]]
    }

    #[must_use]
    pub(crate) fn get_orientation(&self) -> &Quaternion {
        &self.orientation
    }

    /// The camera's orientation as `(roll, pitch, bearing)`.
    #[must_use]
    pub fn get_orientation_angles(&self) -> (f64, f64, f64) {
        let f = self.forward();
        let r = self.right();
        let u = self.up();

        let f01 = (f[0] * f[0] + f[1] * f[1]).sqrt();
        let pitch = f01.atan2(-f[2]);

        let (bearing, roll) =
            if f01 > 1.0e-3 { (-f[0].atan2(-f[1]), -r[2].atan2(u[2])) } else { ((-r[1]).atan2(r[0]), 0.0) };
        (roll, pitch, bearing)
    }

    pub fn set_orientation(&mut self, roll: f64, pitch: f64, bearing: f64) {
        self.orientation = orientation_from_roll_pitch_bearing(roll, pitch, bearing);
        update_transform_orientation(&mut self.transform, &self.orientation);
    }

    pub(crate) fn set_orientation_quaternion(&mut self, orientation: &Quaternion) {
        self.orientation = *orientation;
        update_transform_orientation(&mut self.transform, &self.orientation);
    }

    pub(crate) fn set_position(&mut self, position: &Vec3) {
        update_transform_position(&mut self.transform, position);
    }

    #[must_use]
    pub(crate) fn orientation_from_frame(forward: &Vec3, up: &Vec3) -> Option<Quaternion> {
        let epsilon = 1e-15;

        let right = vec3_cross(up, forward);

        if vec3_length(&right) < epsilon {
            return None;
        }

        let f01 = (forward[0] * forward[0] + forward[1] * forward[1]).sqrt();
        let pitch = f01.atan2(-forward[2]);

        let (bearing, roll) = if f01 > 1.0e-3 {
            (-forward[0].atan2(-forward[1]), -right[2].atan2(up[2]))
        } else {
            ((-right[1]).atan2(right[0]), 0.0)
        };

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
    pub(crate) fn get_location(&self) -> Option<LatLngAltitude> {
        let position_value = self.position?;
        if position_value[1] < 0.0 || position_value[1] > 1.0 {
            return None;
        }

        // C++ builds the LatLng in a noexcept function: a NaN or infinite x
        // throws there (std::terminate); this panics.
        let location = LatLng::new(lat_from_mercator_y(position_value[1]), lng_from_mercator_x(position_value[0]))
            .unwrap_or_else(|e| panic!("{e}"));

        let meters_per_pixel = Projection::get_meters_per_pixel_at_latitude(location.latitude(), 0.0);
        let world_size = Projection::world_size(2.0f64.powf(0.0));
        let altitude = position_value[2] * world_size * meters_per_pixel;

        Some(LatLngAltitude { location, altitude })
    }

    /// Helper function for setting orientation of the camera by defining a
    /// focus point on the map. Up vector is required in certain scenarios
    /// where bearing can't be deduced from the viewing direction.
    /// (`lookAtPoint(const LatLng&, const std::optional<vec3>& upVector = std::nullopt)`.)
    #[cfg(test)]
    pub(crate) fn look_at_point(&mut self, location: &LatLng, up_vector: Option<Vec3>) {
        self.orientation = None;
        let Some(position) = self.position else {
            return;
        };

        let target = to_mercator(location, 0.0);
        let forward = vec3_sub(&target, &position);
        let mut up = up_vector.unwrap_or([0.0, 0.0, 1.0]);

        // Flip z-component of the up vector if it's pointing downwards
        up[2] = up[2].abs();

        let q = Camera::orientation_from_frame(&forward, &up);

        if let Some(q) = q {
            self.orientation = Some(q.m());
        }
    }

    /// Helper function for setting the orientation of the camera as a roll,
    /// pitch, and a bearing. All values are in degrees.
    #[cfg(test)]
    pub(crate) fn set_roll_pitch_bearing(&mut self, roll: f64, pitch: f64, bearing: f64) {
        self.orientation = Some(
            orientation_from_roll_pitch_bearing(
                mln_math::deg2rad(roll),
                mln_math::deg2rad(pitch),
                mln_math::deg2rad(bearing),
            )
            .m(),
        );
    }
}

#[cfg(test)]
#[path = "util_camera_test.rs"]
mod util_camera_test;
