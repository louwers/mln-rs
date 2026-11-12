// Ports src/mln/map/vertical_perspective_projection.hpp and
// src/mln/map/vertical_perspective_projection.cpp.
//
// The C++ class's static functions are associated functions here, and the
// file-local helpers are private functions. The `vec3` overload of the
// file-local `slerp` is `slerp_vec3`.

use std::f64::consts::PI;

use geometry::Point;
use mln_tile_id::UnwrappedTileID;
use mln_util::constants::{EXTENT, LATITUDE_MAX};
use mln_util::error::Error;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::interpolate::interpolate;
use mln_util::mat4::{Mat4, matrix};
use mln_util::projection::Projection;
use mln_util::quaternion::Quaternion;
use mln_util::vectors::{Vec3, Vec4};

use crate::projection_base::{
    ProjectedTilePoint, ProjectionBase, ProjectionData, mercator_tile_coords, mercator_tile_matrix,
};
use crate::transform_state::{NorthOrientation, TransformState};

/// `LatLng{lat, lon, mode}`: C++ constructs it directly, which throws
/// `std::domain_error` on invalid input; nothing catches it there.
fn lat_lng(lat: f64, lon: f64, mode: WrapMode) -> LatLng {
    LatLng::with_mode(lat, lon, mode).unwrap_or_else(|e| panic!("{e}"))
}

/// `std::clamp(value, lo, hi)`: `value < lo ? lo : hi < value ? hi : value`
/// (NaN passes through).
fn clamp(value: f64, lo: f64, hi: f64) -> f64 {
    if value < lo {
        lo
    } else if hi < value {
        hi
    } else {
        value
    }
}

/// How far from the polar axis a unit-sphere point can be and still be on its
/// pole, with no longitude of its own: GL JS
/// `sphereSurfacePointToCoordinates` gives it longitude 0.
const POLE_AXIS_DISTANCE: f64 = 1e-6;

fn rotate_x(v: &Vec3, rad: f64) -> Vec3 {
    let c = rad.cos();
    let s = rad.sin();
    [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c]
}

fn rotate_y(v: &Vec3, rad: f64) -> Vec3 {
    let c = rad.cos();
    let s = rad.sin();
    [v[2] * s + v[0] * c, v[1], v[2] * c - v[0] * s]
}

fn rotate_z(v: &Vec3, rad: f64) -> Vec3 {
    let c = rad.cos();
    let s = rad.sin();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]]
}

fn dot(a: &Vec3, b: &Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length(v: &Vec3) -> f64 {
    dot(v, v).sqrt()
}

fn normalized(v: &Vec3) -> Vec3 {
    let len = length(v);
    [v[0] / len, v[1] / len, v[2] / len]
}

fn scaled(v: &Vec3, s: f64) -> Vec3 {
    [v[0] * s, v[1] * s, v[2] * s]
}

fn add(a: &Vec3, b: &Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// GL JS `VerticalPerspectiveTransform._isLineOfSightBlocked`: whether the
/// planet stands between the camera and a point, both on the unit sphere's
/// scale.
fn line_of_sight_blocked(camera: &Vec3, point: &Vec3) -> bool {
    let to_point = add(point, &scaled(camera, -1.0));
    let length_sq = dot(&to_point, &to_point);
    if length_sq == 0.0 {
        return false;
    }
    let t = clamp(-dot(camera, &to_point) / length_sq, 0.0, 1.0);
    let closest = add(camera, &scaled(&to_point, t));
    dot(&closest, &closest) < 1.0
}

/// C++ also carries `tMax`, which nothing reads.
struct RaySphereIntersection {
    t_min: f64,
}

/// GL JS `raySphereIntersection`: the numerically stable form for a unit
/// sphere at the origin.
fn ray_sphere_intersection(origin: &Vec3, direction: &Vec3) -> Option<RaySphereIntersection> {
    let origin_dot_direction = dot(origin, direction);
    let inner = add(origin, &scaled(direction, -origin_dot_direction));
    let discriminant = 1.0 - dot(&inner, &inner);
    // A NaN ray, from a NaN point, hits nothing.
    if discriminant.is_nan() || discriminant < 0.0 {
        return None;
    }
    let c = dot(origin, origin) - 1.0;
    let q = -origin_dot_direction + (if origin_dot_direction < 0.0 { 1.0 } else { -1.0 }) * discriminant.sqrt();
    let t0 = c / q;
    let t1 = q;
    // Both behind the ray's origin, as for a ray into the sky past a nearby
    // planet: no intersection.
    if f64::max(t0, t1) < 0.0 {
        return None;
    }
    Some(RaySphereIntersection { t_min: f64::min(t0, t1) })
}

fn point_plane_signed_distance(plane: &Vec4, point: &Vec3) -> f64 {
    plane[0] * point[0] + plane[1] * point[1] + plane[2] * point[2] + plane[3]
}

fn ray_direction_from_pixel(state: &TransformState, point: &ScreenCoordinate, inverse_view_projection: &Mat4) -> Vec3 {
    let size = state.get_size();
    let clip: Vec4 =
        [point.x / f64::from(size.width) * 2.0 - 1.0, point.y / f64::from(size.height) * 2.0 - 1.0, 1.0, 1.0];
    let world = matrix::transform_mat4_value(&clip, inverse_view_projection);
    let target: Vec3 = [world[0] / world[3], world[1] / world[3], world[2] / world[3]];
    normalized(&add(&target, &scaled(&state.get_globe_camera_position(), -1.0)))
}

/// Angle to rotate the vector (x0, y0) onto the direction of (x1, y1).
fn angle_to_rotate_between_vectors_2d(mut x0: f64, mut y0: f64, mut x1: f64, mut y1: f64) -> f64 {
    let len0 = (x0 * x0 + y0 * y0).sqrt();
    let len1 = (x1 * x1 + y1 * y1).sqrt();
    x0 /= len0;
    y0 /= len0;
    x1 /= len1;
    y1 /= len1;
    let cross = x0 * y1 - x1 * y0;
    let angle = clamp(x0 * x1 + y0 * y1, -1.0, 1.0).acos();
    if cross > 0.0 { angle } else { -angle }
}

fn distance_of_angles_radians(a: f64, b: f64) -> f64 {
    let two_pi = 2.0 * PI;
    // `%` on floats is C fmod.
    let d = ((a - b) % two_pi + two_pi) % two_pi;
    f64::min(d, two_pi - d)
}

/// The integral of 1 / cos(x): where the longitude gets to when the globe
/// turns at a steady rate.
fn integrate_secant(x: f64) -> f64 {
    let half = 0.5 * x;
    let sin = half.sin();
    let cos = half.cos();
    (sin + cos).ln() - (cos - sin).ln()
}

fn sign(value: f64) -> i32 {
    i32::from(value > 0.0) - i32::from(value < 0.0)
}

/// The globe: tile geometry projected onto a unit sphere, viewed with a
/// vertical perspective camera. Port of MapLibre GL JS
/// `vertical_perspective_transform.ts`.
#[derive(Clone, Copy, Debug, Default)]
pub struct VerticalPerspectiveProjection;

/// `VerticalPerspectiveProjection::CenterAndBearing`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CenterAndBearing {
    pub center: LatLng,
    /// In degrees, with `CameraOptions`'s sign.
    pub bearing: f64,
}

// GL JS vertical_perspective_camera_helper constants.
const RAY_SURFACE_DISTANCE_FOR_SLOWING_START: f64 = 0.3;
const SLOWING_MULTIPLIER: f64 = 0.5;
const INTERPOLATE_TO_HEURISTIC_START_LNG: f64 = 45.0;
const INTERPOLATE_TO_HEURISTIC_END_LNG: f64 = 85.0;
const INTERPOLATE_TO_HEURISTIC_EXPONENT: f64 = 0.25;
const INTERPOLATE_TO_HEURISTIC_START_HORIZON: f64 = 0.95;
const INTERPOLATE_TO_HEURISTIC_END_HORIZON: f64 = 0.999;
const SLOWING_RADIUS_START: f64 = 0.9;
const SLOWING_RADIUS_STOP: f64 = 0.5;
const SLOWING_RADIUS_SLOW_FACTOR: f64 = 0.25;
// GL JS vertical_perspective_transform: how many times the view the area past
// the Mercator range has grown to when the center is back on its edge.
const POLE_AREA_FADE_SCALE: f64 = 4.0;

fn remap_saturate(value: f64, old_min: f64, old_max: f64, new_min: f64, new_max: f64) -> f64 {
    let t = clamp((value - old_min) / (old_max - old_min), 0.0, 1.0);
    new_min + (new_max - new_min) * t
}

impl VerticalPerspectiveProjection {
    /// GL JS `earthRadius`, `GLOBE_RADIUS` in the shaders: the meters an
    /// elevation is measured against.
    pub const GLOBE_RADIUS_METERS: f64 = 6_371_008.8;

    /// Globe radius in pixels at the given world size and center latitude, so
    /// zoom means the same on both projections.
    #[must_use]
    pub fn globe_radius_pixels(world_size: f64, center_latitude: f64) -> f64 {
        world_size / (2.0 * PI) / Self::planet_scale(center_latitude)
    }

    /// GL JS `planetScaleAtLatitude`: the cosine of the center latitude the
    /// globe radius is divided by. Mercator ends at the edge of its range, so
    /// past it the globe keeps its size there.
    #[must_use]
    pub fn planet_scale(center_latitude: f64) -> f64 {
        mln_math::deg2rad(f64::min(center_latitude.abs(), LATITUDE_MAX)).cos()
    }

    /// The unit-sphere position of a point in tile units.
    #[must_use]
    pub fn tile_coordinates_to_sphere(tile_point: &Point<f64>, tile_id: &UnwrappedTileID) -> Vec3 {
        let scale = 1.0 / (1u64 << tile_id.canonical.z) as f64;
        let mercator_x = tile_point.x / f64::from(EXTENT) * scale + f64::from(tile_id.canonical.x) * scale;
        let mercator_y = tile_point.y / f64::from(EXTENT) * scale + f64::from(tile_id.canonical.y) * scale;
        let spherical_x = mercator_x * PI * 2.0 + PI;
        // sin/cos of the latitude from the Mercator Y through the tangent
        // half-angle identities, as the shaders do.
        let t = (PI - (mercator_y * PI * 2.0)).exp();
        let t2 = t * t;
        let sin_y = (t2 - 1.0) / (t2 + 1.0);
        let cos_y = (2.0 * t) / (t2 + 1.0);
        [spherical_x.sin() * cos_y, sin_y, spherical_x.cos() * cos_y]
    }

    /// The view-projection matrix that takes unit-sphere positions to clip
    /// space.
    pub fn globe_view_projection_matrix(state: &TransformState, radius: f64) -> Mat4 {
        let size = state.get_size();
        let camera_to_center_distance = f64::from(state.get_camera_to_center_distance());
        let center = state.get_lat_lng(WrapMode::Unwrapped);

        let mut m: Mat4 = [0.0; 16];
        matrix::perspective(
            &mut m,
            f64::from(state.get_field_of_view()),
            f64::from(size.width) / f64::from(size.height),
            0.5,
            camera_to_center_distance + radius * 2.0,
        );

        let offset = state.get_center_offset();
        m[8] = -offset.x * 2.0 / f64::from(size.width);
        m[9] = offset.y * 2.0 / f64::from(size.height);

        if state.get_north_orientation() != NorthOrientation::Upwards {
            let a = m;
            matrix::rotate_z(&mut m, &a, -state.get_north_orientation_angle());
        }

        let a = m;
        matrix::translate(&mut m, &a, 0.0, 0.0, -camera_to_center_distance);
        let a = m;
        matrix::rotate_z(&mut m, &a, state.get_roll());
        let a = m;
        matrix::rotate_x(&mut m, &a, -state.get_pitch());
        let a = m;
        matrix::rotate_z(&mut m, &a, -state.get_bearing());
        let a = m;
        matrix::translate(&mut m, &a, 0.0, 0.0, -radius);
        let a = m;
        matrix::rotate_x(&mut m, &a, mln_math::deg2rad(center.latitude()));
        let a = m;
        matrix::rotate_y(&mut m, &a, -mln_math::deg2rad(center.longitude()));
        let a = m;
        matrix::scale(&mut m, &a, radius, radius, radius);
        m
    }

    /// The plane (as ax + by + cz + d) that separates the visible hemisphere
    /// from the back of the globe.
    pub fn clipping_plane(state: &TransformState, radius: f64) -> Vec4 {
        let pitch = state.get_pitch();
        let distance_camera_to_b = f64::from(state.get_camera_to_center_distance()) / radius;
        let distance_camera_to_a = pitch.sin() * distance_camera_to_b;
        let distance_a_to_c = pitch.cos() * distance_camera_to_b + 1.0;
        let distance_camera_to_c =
            (distance_camera_to_a * distance_camera_to_a + distance_a_to_c * distance_a_to_c).sqrt();
        let tangent_plane_distance_to_c = 1.0 / distance_camera_to_c;

        let mut plane: Vec3 =
            [0.0, -distance_camera_to_a / distance_camera_to_c, distance_a_to_c / distance_camera_to_c];

        let center = state.get_lat_lng(WrapMode::Unwrapped);
        plane = rotate_z(&plane, state.get_bearing());
        plane = rotate_x(&plane, -mln_math::deg2rad(center.latitude()));
        plane = rotate_y(&plane, mln_math::deg2rad(center.longitude()));

        let scale = 1.0 / (plane[0] * plane[0] + plane[1] * plane[1] + plane[2] * plane[2]).sqrt();
        [plane[0] * scale, plane[1] * scale, plane[2] * scale, -tangent_plane_distance_to_c * scale]
    }

    /// Camera position in unit-sphere space.
    pub fn camera_position(state: &TransformState, radius: f64) -> Vec3 {
        let center = state.get_lat_lng(WrapMode::Unwrapped);
        let mut position: Vec3 = [0.0, 0.0, f64::from(state.get_camera_to_center_distance()) / radius];
        position = rotate_z(&position, -state.get_roll());
        position = rotate_x(&position, state.get_pitch());
        position = rotate_z(&position, state.get_bearing());
        position = add(&position, &[0.0, 0.0, 1.0]);
        position = rotate_x(&position, -mln_math::deg2rad(center.latitude()));
        position = rotate_y(&position, mln_math::deg2rad(center.longitude()));
        position
    }

    #[must_use]
    pub fn surface_vector(lat_lng: &LatLng) -> Vec3 {
        let lng = mln_math::deg2rad(lat_lng.longitude());
        let lat = mln_math::deg2rad(lat_lng.latitude());
        let len = lat.cos();
        [lng.sin() * len, lat.sin(), lng.cos() * len]
    }

    #[must_use]
    pub fn surface_vector_to_lat_lng(surface: &Vec3) -> LatLng {
        let latitude = mln_math::rad2deg(clamp(surface[1], -1.0, 1.0).asin());
        let length_xz = (surface[0] * surface[0] + surface[2] * surface[2]).sqrt();
        if length_xz <= POLE_AXIS_DISTANCE {
            return lat_lng(latitude, 0.0, WrapMode::Unwrapped);
        }
        let acos_z = clamp(surface[2] / length_xz, -1.0, 1.0).acos();
        let longitude = mln_math::rad2deg(if surface[0] > 0.0 { acos_z } else { -acos_z });
        lat_lng(latitude, longitude, WrapMode::Wrapped)
    }

    /// Unit-sphere point under a screen pixel (y up, as `TransformState`
    /// takes it); pixels off the globe snap to the nearest point on the
    /// horizon.
    pub fn screen_coordinate_to_surface(state: &TransformState, point: &ScreenCoordinate) -> Vec3 {
        let origin = state.get_globe_camera_position();
        let direction = ray_direction_from_pixel(state, point, &state.get_inverse_globe_view_projection_matrix());

        if let Some(intersection) = ray_sphere_intersection(&origin, &direction) {
            return normalized(&add(&origin, &scaled(&direction, intersection.t_min)));
        }

        // The ray misses the globe: take the nearest point on the horizon, the
        // circle where the clipping plane cuts the sphere.
        let plane = state.get_globe_clipping_plane();
        let direction_dot_plane = plane[0] * direction[0] + plane[1] * direction[1] + plane[2] * direction[2];
        let origin_to_plane = point_plane_signed_distance(&plane, &origin);
        let distance_to_intersection = -origin_to_plane / direction_dot_plane;

        let plane_intersection = if distance_to_intersection > 0.0 {
            add(&origin, &scaled(&direction, distance_to_intersection))
        } else {
            const MAX_RAY_LENGTH: f64 = 2.0;
            let distant_point = add(&origin, &scaled(&direction, MAX_RAY_LENGTH));
            let distance_from_plane = point_plane_signed_distance(&plane, &distant_point);
            add(&distant_point, &scaled(&[plane[0], plane[1], plane[2]], -distance_from_plane))
        };

        let horizon_center = scaled(&[plane[0], plane[1], plane[2]], -plane[3]);
        let horizon_radius = f64::max(0.0, 1.0 - plane[3] * plane[3]).sqrt();
        let relative = add(&plane_intersection, &scaled(&horizon_center, -1.0));
        add(&horizon_center, &scaled(&relative, horizon_radius / length(&relative)))
    }

    /// Whether a screen pixel (y up) looks at the globe rather than past it.
    pub fn screen_coordinate_hits_globe(state: &TransformState, point: &ScreenCoordinate) -> bool {
        let direction = ray_direction_from_pixel(state, point, &state.get_inverse_globe_view_projection_matrix());
        ray_sphere_intersection(&state.get_globe_camera_position(), &direction).is_some()
    }

    pub fn screen_coordinate_to_lat_lng(
        state: &TransformState,
        point: &ScreenCoordinate,
        wrap_mode: WrapMode,
    ) -> LatLng {
        let lat_lng = Self::surface_vector_to_lat_lng(&Self::screen_coordinate_to_surface(state, point));
        if wrap_mode == WrapMode::Wrapped { lat_lng.wrapped() } else { lat_lng }
    }

    pub fn lat_lng_to_screen_coordinate(state: &TransformState, lat_lng: &LatLng, clip: &mut Vec4) -> ScreenCoordinate {
        let surface = Self::surface_vector(lat_lng);
        let position: Vec4 = [surface[0], surface[1], surface[2], 1.0];
        matrix::transform_mat4(clip, &position, &state.get_globe_view_projection_matrix());
        let size = state.get_size();
        Point::new(
            (clip[0] / clip[3] * 0.5 + 0.5) * f64::from(size.width),
            (clip[1] / clip[3] * 0.5 + 0.5) * f64::from(size.height),
        )
    }

    /// The center that puts `lat_lng` under `anchor` with the bearing
    /// unchanged, if one exists.
    pub fn center_for_location_at_point(
        state: &TransformState,
        lat_lng: &LatLng,
        anchor: &ScreenCoordinate,
    ) -> Option<LatLng> {
        // GL JS `setLocationAtPoint`: solve the rotation that carries the
        // surface point under the pixel onto the target.
        const LIMIT: f64 = PI * 0.5;
        let center = state.get_lat_lng(WrapMode::Unwrapped);
        let vec_to_pixel_current = Self::screen_coordinate_to_surface(state, anchor);
        let vec_to_target = Self::surface_vector(lat_lng);

        let mut rotated_pixel_vector = rotate_y(&vec_to_pixel_current, -mln_math::deg2rad(center.longitude()));
        rotated_pixel_vector = rotate_x(&rotated_pixel_vector, mln_math::deg2rad(center.latitude()));

        let vec_to_target_xz_length_squared = vec_to_target[0] * vec_to_target[0] + vec_to_target[2] * vec_to_target[2];
        let target_x_squared = rotated_pixel_vector[0] * rotated_pixel_vector[0];
        if vec_to_target_xz_length_squared < target_x_squared {
            return None;
        }

        let intersection_a = (vec_to_target_xz_length_squared - target_x_squared).sqrt();
        let intersection_b = -intersection_a;

        let solve = |intersection: f64| {
            let lng = angle_to_rotate_between_vectors_2d(
                vec_to_target[0],
                vec_to_target[2],
                rotated_pixel_vector[0],
                intersection,
            );
            let vec_to_target_lng = rotate_y(&vec_to_target, -lng);
            let lat = angle_to_rotate_between_vectors_2d(
                vec_to_target_lng[1],
                vec_to_target_lng[2],
                rotated_pixel_vector[1],
                rotated_pixel_vector[2],
            );
            (lng, lat)
        };
        let (lng_a, lat_a) = solve(intersection_a);
        let (lng_b, lat_b) = solve(intersection_b);

        let valid_a = (-LIMIT..=LIMIT).contains(&lat_a);
        let valid_b = (-LIMIT..=LIMIT).contains(&lat_b);

        let (lng, lat) = if valid_a && valid_b {
            let center_lng = mln_math::deg2rad(center.longitude());
            let center_lat = mln_math::deg2rad(center.latitude());
            let distance_a =
                distance_of_angles_radians(lng_a, center_lng) + distance_of_angles_radians(lat_a, center_lat);
            let distance_b =
                distance_of_angles_radians(lng_b, center_lng) + distance_of_angles_radians(lat_b, center_lat);
            if distance_a < distance_b { (lng_a, lat_a) } else { (lng_b, lat_b) }
        } else if valid_a {
            (lng_a, lat_a)
        } else if valid_b {
            (lng_b, lat_b)
        } else {
            return None;
        };
        Some(self::lat_lng(clamp(mln_math::rad2deg(lat), -90.0, 90.0), mln_math::rad2deg(lng), WrapMode::Unwrapped))
    }

    /// GL JS `handleMapControlsRollPitchBearingZoom`: after the zoom changed
    /// by `zoom_delta`, move the center so that `zoom_location` stays under
    /// `anchor`, exactly where that is well posed and by a damped heuristic
    /// near the horizon, where a pixel is worth degrees of arc and the exact
    /// solution jumps or does not exist. With `free_bearing` the planet turns
    /// as one body instead and the bearing follows it, so near a pole, where
    /// the longitude of the center swings, the view does not spin: the exact
    /// part is the rotation that brings `zoom_location` back under `anchor`,
    /// the heuristic part moves the center along the great circle towards
    /// it. (C++ defaults `freeBearing` to false.)
    pub fn zoom_around_point(
        state: &mut TransformState,
        anchor: &ScreenCoordinate,
        zoom_location: &LatLng,
        zoom_delta: f64,
        free_bearing: bool,
    ) {
        if zoom_delta == 0.0 {
            return;
        }
        let center = state.get_lat_lng(WrapMode::Unwrapped);
        // A location on a pole has no longitude of its own, so the zoom keeps
        // the center's meridian rather than turning the planet about the pole
        // towards the longitude the location was given.
        let on_pole = mln_math::deg2rad(zoom_location.latitude()).cos() <= POLE_AXIS_DISTANCE;
        let d_lng_raw = if on_pole {
            0.0
        } else {
            Self::difference_of_angles_degrees(center.longitude(), zoom_location.longitude())
        };
        let d_lng = d_lng_raw / ((d_lng_raw / 180.0).abs() + 1.0);
        let d_lat = Self::difference_of_angles_degrees(center.latitude(), zoom_location.latitude());

        // Slow the movement down when the anchor's ray passes far from the
        // planet.
        let ray_origin = state.get_globe_camera_position();
        let ray_direction = ray_direction_from_pixel(state, anchor, &state.get_inverse_globe_view_projection_matrix());
        let closest_point = add(&ray_origin, &scaled(&ray_direction, -dot(&ray_origin, &ray_direction)));
        let ray_distance_from_globe_center = length(&closest_point);
        let distance_factor =
            (-f64::max(ray_distance_from_globe_center - 1.0 - RAY_SURFACE_DISTANCE_FOR_SLOWING_START, 0.0)
                * SLOWING_MULTIPLIER)
                .exp();
        let interpolation_factor_horizon = remap_saturate(
            ray_distance_from_globe_center,
            INTERPOLATE_TO_HEURISTIC_START_HORIZON,
            INTERPOLATE_TO_HEURISTIC_END_HORIZON,
            0.0,
            1.0,
        );

        // And when the globe is small on the viewport, where that is a
        // stand-in for the anchor being near the horizon.
        let size = state.get_size();
        let radius = Self::globe_radius_pixels(Projection::world_size(state.get_scale()), center.latitude())
            / f64::from(size.width.min(size.height));
        let radius_factor =
            remap_saturate(radius, SLOWING_RADIUS_START, SLOWING_RADIUS_STOP, 1.0, SLOWING_RADIUS_SLOW_FACTOR);
        let slowing_factor = f64::min(distance_factor, 1.0 + (radius_factor - 1.0) * interpolation_factor_horizon);

        let factor = (1.0 - 2.0f64.powf(-zoom_delta)) * slowing_factor;
        if free_bearing {
            zoom_turning_the_planet(state, anchor, zoom_location, factor, interpolation_factor_horizon);
            return;
        }
        let heuristic_center = lat_lng(
            clamp(center.latitude() + d_lat * factor, -90.0, 90.0),
            center.longitude() + d_lng * factor,
            WrapMode::Unwrapped,
        );

        let mut exact_center = Self::center_for_location_at_point(state, zoom_location, anchor).unwrap_or(center);
        if on_pole {
            exact_center = lat_lng(exact_center.latitude(), center.longitude(), WrapMode::Unwrapped);
        }

        let interpolation_factor_longitude = remap_saturate(
            d_lng_raw.abs(),
            INTERPOLATE_TO_HEURISTIC_START_LNG,
            INTERPOLATE_TO_HEURISTIC_END_LNG,
            0.0,
            1.0,
        );
        let heuristic_factor = f64::max(interpolation_factor_longitude, interpolation_factor_horizon)
            .powf(INTERPOLATE_TO_HEURISTIC_EXPONENT);
        let target = lat_lng(
            clamp(
                exact_center.latitude()
                    + Self::difference_of_angles_degrees(exact_center.latitude(), heuristic_center.latitude())
                        * heuristic_factor,
                -90.0,
                90.0,
            ),
            exact_center.longitude()
                + Self::difference_of_angles_degrees(exact_center.longitude(), heuristic_center.longitude())
                    * heuristic_factor,
            WrapMode::Unwrapped,
        );
        let constrained = state.constrained_center(&target, state.get_zoom());
        let zoom = state.get_zoom() + Self::zoom_adjustment(center.latitude(), constrained.latitude());
        state.set_lat_lng_zoom(&constrained, zoom);
    }

    /// Zoom change that keeps the globe the same apparent size when the
    /// center moves between latitudes.
    #[must_use]
    pub fn zoom_adjustment(from_latitude: f64, to_latitude: f64) -> f64 {
        (Self::planet_scale(to_latitude) / Self::planet_scale(from_latitude)).log2()
    }

    /// GL JS `_getMaxLatitude`: how close to a pole the center can be at the
    /// given zoom. Past the Mercator range there are no tiles, so the center
    /// reaches the pole only while that area fits in the view, and eases back
    /// to the edge of the range as the area outgrows the view.
    pub fn max_latitude(state: &TransformState, zoom: f64) -> f64 {
        let area_radians = mln_math::deg2rad(90.0 - LATITUDE_MAX);
        let area_pixels =
            Self::globe_radius_pixels(Projection::world_size(2.0f64.powf(zoom)), LATITUDE_MAX) * area_radians;
        let size = state.get_size();
        let reach_pixels = f64::from(size.width.min(size.height)) / 2.0;
        let center_pixels = f64::min(area_pixels, reach_pixels)
            * remap_saturate(area_pixels, reach_pixels, reach_pixels * POLE_AREA_FADE_SCALE, 1.0, 0.0);
        LATITUDE_MAX + mln_math::rad2deg(area_radians * center_pixels / area_pixels)
    }

    /// GL JS `computeGlobePanCenter`: the center after a pan of `offset`
    /// pixels, turning the planet by degrees per pixel at the center's
    /// latitude. The longitude is unwrapped and the latitude clamped to the
    /// poles.
    pub fn pan_center(state: &TransformState, offset: &ScreenCoordinate) -> LatLng {
        // GL JS turns the pan by its bearing, which is the state's with the
        // sign flipped.
        let bearing = -state.get_bearing();
        let x = offset.x * bearing.cos() - offset.y * bearing.sin();
        let y = offset.x * bearing.sin() + offset.y * bearing.cos();

        let center = state.get_lat_lng(WrapMode::Unwrapped);
        let latitude = center.latitude();
        let inverse_scale = |lat: f64| 1.0 / Self::planet_scale(lat);
        // Longitude speeds up towards the poles, where the planet is
        // narrower, but as the globe zooms out from 7 to 3 the latitude it
        // speeds up for is capped at 60°: GL JS's numbers.
        let zoomed_out = clamp((7.0 - (state.get_zoom() + Self::zoom_adjustment(latitude, 0.0))) / 4.0, 0.0, 1.0);
        let longitude_speed =
            interpolate(&inverse_scale(latitude), &inverse_scale(f64::min(latitude.abs(), 60.0)), zoomed_out);
        let degrees_per_pixel =
            360.0 / (2.0 * PI * Self::globe_radius_pixels(Projection::world_size(state.get_scale()), latitude));
        lat_lng(
            clamp(latitude + y * degrees_per_pixel, -90.0, 90.0),
            center.longitude() - x * degrees_per_pixel * longitude_speed,
            WrapMode::Unwrapped,
        )
    }

    /// GL JS `versorSetLocationAtPoint` without the fixed bearing: the whole
    /// rotation that brings the location under `point - pan_delta` under
    /// `point` (both y up), so the grabbed location tracks the cursor exactly
    /// and the bearing turns with the planet, over the poles too. The
    /// longitude stays unwrapped around the current center, the latitude is
    /// not yet constrained.
    pub fn drag_camera(
        state: &TransformState,
        point: &ScreenCoordinate,
        pan_delta: &ScreenCoordinate,
    ) -> CenterAndBearing {
        center_and_bearing(
            &turned(
                &camera_orientation(state),
                &pan_surface_vector(state, &Point::new(point.x - pan_delta.x, point.y - pan_delta.y)),
                &pan_surface_vector(state, point),
            ),
            &state.get_lat_lng(WrapMode::Unwrapped),
        )
    }

    /// Where a fling of `offset` pixels (y up) ends with the bearing free:
    /// the planet turns about the axis a drag in that direction at `center`
    /// (y up) turns it about, by GL JS's degrees per pixel at the center's
    /// latitude and short of half a turn, the orientation carried along.
    pub fn fling_camera(
        state: &TransformState,
        center: &ScreenCoordinate,
        offset: &ScreenCoordinate,
    ) -> CenterAndBearing {
        let lat_lng = state.get_lat_lng(WrapMode::Unwrapped);
        let pixels = offset.x.hypot(offset.y);
        if pixels == 0.0 {
            return CenterAndBearing { center: lat_lng, bearing: -mln_math::rad2deg(state.get_bearing()) };
        }
        let axis = cross(
            &pan_surface_vector(state, &Point::new(center.x - offset.x / pixels, center.y - offset.y / pixels)),
            &pan_surface_vector(state, center),
        );
        let degrees_per_pixel = 360.0
            / (2.0 * PI * Self::globe_radius_pixels(Projection::world_size(state.get_scale()), lat_lng.latitude()));
        center_and_bearing(
            &camera_orientation(state)
                .multiply(&rotation(&axis, mln_math::deg2rad(f64::min(pixels * degrees_per_pixel, 179.5)))),
            &lat_lng,
        )
    }

    /// The camera `t` of the way from one camera to another, the planet
    /// turning about one axis.
    #[must_use]
    pub fn interpolate_camera(from: &CenterAndBearing, to: &CenterAndBearing, t: f64) -> CenterAndBearing {
        center_and_bearing(
            &slerp(
                &orientation_from_lat_lng_bearing(&from.center, from.bearing),
                &orientation_from_lat_lng_bearing(&to.center, to.bearing),
                t,
            ),
            &from.center,
        )
    }

    /// GL JS `versorSetLocationAtPoint` with the bearing held, as a drag uses
    /// it: the center after the one rotation that brings the location under
    /// `point - pan_delta` back under `point` (both y up). Within 12 degrees
    /// of the pole's edge the cursor turns a dial around the pole, and a
    /// swing over a pole stops on it. The longitude stays unwrapped around
    /// the current center, the latitude is not yet constrained.
    pub fn drag_center(state: &TransformState, point: &ScreenCoordinate, pan_delta: &ScreenCoordinate) -> LatLng {
        let center = state.get_lat_lng(WrapMode::Unwrapped);
        // GL JS's bearing is the state's with the sign flipped.
        let bearing = -mln_math::rad2deg(state.get_bearing());
        let swung = orientation_to_lat_lng_bearing(&turned(
            &camera_orientation(state),
            &pan_surface_vector(state, &Point::new(point.x - pan_delta.x, point.y - pan_delta.y)),
            &pan_surface_vector(state, point),
        ));

        // A swing over a pole turns the bearing around; with the bearing held,
        // the center stops on that pole.
        let latitude = if Self::difference_of_angles_degrees(bearing, swung.bearing).abs() <= 90.0 {
            swung.latitude
        } else if swung.latitude > 0.0 {
            90.0
        } else {
            -90.0
        };

        // The swing's longitude is ill-conditioned near the pole, so within
        // the last 12 degrees before the pole's edge the cursor turns a dial
        // around the pole instead, blended in with a smoothstep and in full
        // from the edge on. GL JS takes the sweep from the screen with y down.
        let pole_latitude = if center.latitude() >= 0.0 { 90.0 } else { -90.0 };
        let mut clip: Vec4 = [0.0; 4];
        let pole =
            Self::lat_lng_to_screen_coordinate(state, &lat_lng(pole_latitude, 0.0, WrapMode::Unwrapped), &mut clip);
        let rx = point.x - pole.x;
        let ry = pole.y - point.y;
        let ramp = clamp(1.0 - (LATITUDE_MAX - center.latitude().abs()) / DIAL_RAMP_DEGREES, 0.0, 1.0);
        let dial = ramp * ramp * (3.0 - 2.0 * ramp);
        let d_lng_swing = mln_math::wrap(swung.longitude - center.longitude() + 180.0, 0.0, 360.0) - 180.0;
        let mut d_lng_dial = 0.0;
        if dial > 0.0 {
            let radius_squared = f64::max(rx * rx + ry * ry, DIAL_MIN_RADIUS_PIXELS * DIAL_MIN_RADIUS_PIXELS);
            let d_theta = (rx * -pan_delta.y - ry * pan_delta.x) / radius_squared;
            d_lng_dial = (if pole_latitude > 0.0 { 1.0 } else { -1.0 }) * mln_math::rad2deg(d_theta);
        }
        lat_lng(latitude, center.longitude() + (1.0 - dial) * d_lng_swing + dial * d_lng_dial, WrapMode::Unwrapped)
    }

    /// The shorter signed way around from one angle to the other, in degrees.
    #[must_use]
    pub fn difference_of_angles_degrees(from: f64, to: f64) -> f64 {
        let a = mln_math::wrap(from, 0.0, 360.0);
        let b = mln_math::wrap(to, 0.0, 360.0);
        let direct = b - a;
        let around = if b > a { direct - 360.0 } else { direct + 360.0 };
        if direct.abs() < around.abs() { direct } else { around }
    }

    /// Distance between two locations along the surface, in pixels at the
    /// given world size and center latitude.
    #[must_use]
    pub fn surface_distance_pixels(world_size: f64, center_latitude: f64, a: &LatLng, b: &LatLng) -> f64 {
        let radians = clamp(dot(&Self::surface_vector(a), &Self::surface_vector(b)), -1.0, 1.0).acos();
        radians * Self::globe_radius_pixels(world_size, center_latitude)
    }

    /// The location `t` of the way from `start` to `start + delta`, the
    /// longitude paced at 1 / cos(latitude) so the globe appears to turn at a
    /// steady rate. The pace grows without bound towards the poles, so past
    /// the Mercator range it keeps its value at the edge.
    #[must_use]
    pub fn interpolate_lat_lng(start: &LatLng, delta_latitude: f64, delta_longitude: f64, t: f64) -> LatLng {
        let latitude = clamp(start.latitude() + delta_latitude * t, -90.0, 90.0);
        let start_latitude = clamp(start.latitude(), -LATITUDE_MAX, LATITUDE_MAX);
        let end_latitude = clamp(start.latitude() + delta_latitude, -LATITUDE_MAX, LATITUDE_MAX);
        if (end_latitude - start_latitude).abs() <= 1.0 {
            return lat_lng(latitude, start.longitude() + delta_longitude * t, WrapMode::Unwrapped);
        }
        let on_different_hemispheres = sign(end_latitude) != sign(start_latitude);
        let sample_start =
            mln_math::deg2rad(if on_different_hemispheres { -start_latitude.abs() } else { start_latitude.abs() });
        let sample_end = mln_math::deg2rad(end_latitude.abs());
        let value_start = integrate_secant(sample_start);
        let value_end = integrate_secant(sample_end);
        let value_t = integrate_secant(sample_start + t * (sample_end - sample_start));
        let longitude_t = (value_t - value_start) / (value_end - value_start);
        lat_lng(latitude, start.longitude() + delta_longitude * longitude_t, WrapMode::Unwrapped)
    }
}

// GL JS globe_utils drag constants.
const PAN_FALLOFF_BAND: f64 = 0.1;
const DIAL_MIN_RADIUS_PIXELS: f64 = 20.0;
const DIAL_RAMP_DEGREES: f64 = 12.0;
const PAN_MAX_ANGLE: f64 = PI * 0.98;

fn cross(a: &Vec3, b: &Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// GL JS `orientationFromLngLatBearing`: gl-matrix's
/// `quat.fromEuler(-lng, -lat, bearing)`, bearing in GL JS's sign.
fn orientation_from_lat_lng_bearing(center: &LatLng, bearing_degrees: f64) -> Quaternion {
    Quaternion::from_euler_angles(
        mln_math::deg2rad(-center.longitude()),
        mln_math::deg2rad(-center.latitude()),
        mln_math::deg2rad(bearing_degrees),
    )
}

struct Orientation {
    latitude: f64,
    longitude: f64,
    bearing: f64,
}

/// GL JS `lngLatBearingFromOrientation`.
fn orientation_to_lat_lng_bearing(q: &Quaternion) -> Orientation {
    Orientation {
        latitude: -mln_math::rad2deg(clamp(2.0 * (q.w * q.y - q.z * q.x), -1.0, 1.0).asin()),
        longitude: -mln_math::rad2deg((2.0 * (q.w * q.x + q.y * q.z)).atan2(1.0 - 2.0 * (q.x * q.x + q.y * q.y))),
        bearing: mln_math::rad2deg((2.0 * (q.w * q.z + q.x * q.y)).atan2(1.0 - 2.0 * (q.y * q.y + q.z * q.z))),
    }
}

/// GL JS `panSurfaceLocation`: the surface point a drag towards `point` aims
/// at. Exact while the ray is well inside the silhouette, then eased off along
/// a hyperbola that matches it in value and slope and saturates short of the
/// far side, so the drag keeps going once the cursor leaves the planet.
fn pan_surface_vector(state: &TransformState, point: &ScreenCoordinate) -> Vec3 {
    let origin = state.get_globe_camera_position();
    let distance = length(&origin);
    if distance <= 1.0 {
        return VerticalPerspectiveProjection::screen_coordinate_to_surface(state, point);
    }
    let u = normalized(&origin);
    let direction = ray_direction_from_pixel(state, point, &state.get_inverse_globe_view_projection_matrix());
    let c = -dot(&direction, &u);
    let lateral = add(&direction, &scaled(&u, c));
    let s = length(&lateral);
    if s < 1e-9 {
        return VerticalPerspectiveProjection::screen_coordinate_to_surface(state, point);
    }
    let angle = s.atan2(c);
    let handover_angle = (1.0 / distance).asin() * (1.0 - PAN_FALLOFF_BAND);
    if angle < handover_angle {
        return VerticalPerspectiveProjection::screen_coordinate_to_surface(state, point);
    }
    let sin_handover = distance * handover_angle.sin();
    let target_at_handover = clamp(sin_handover, -1.0, 1.0).asin() - handover_angle;
    let slope_at_handover =
        distance * handover_angle.cos() / f64::max(1.0 - sin_handover * sin_handover, 1e-12).sqrt() - 1.0;
    let room = PAN_MAX_ANGLE - target_at_handover;
    let excess = angle - handover_angle;
    let target = clamp(
        target_at_handover + room * (slope_at_handover * excess) / (room + slope_at_handover * excess),
        0.0,
        PAN_MAX_ANGLE,
    );
    normalized(&add(&scaled(&u, target.cos()), &scaled(&lateral, target.sin() / s)))
}

/// The globe camera's orientation; GL JS's bearing is the state's with the
/// sign flipped.
fn camera_orientation(state: &TransformState) -> Quaternion {
    orientation_from_lat_lng_bearing(&state.get_lat_lng(WrapMode::Unwrapped), -mln_math::rad2deg(state.get_bearing()))
}

/// The rotation about `axis` by `angle`, composed as GL JS's drag composes
/// it: the axis is in the surface-vector frame and the orientation in the
/// Euler frame, hence the swizzle.
fn rotation(axis: &Vec3, angle: f64) -> Quaternion {
    let l = length(axis);
    if l == 0.0 {
        return Quaternion::IDENTITY;
    }
    let s = (angle / 2.0).sin();
    Quaternion::new(axis[1] / l * s, -axis[0] / l * s, axis[2] / l * s, (angle / 2.0).cos())
}

/// The orientation after the rotation that brings the surface location
/// `from` to where `to` is.
fn turned(orientation: &Quaternion, from: &Vec3, to: &Vec3) -> Quaternion {
    orientation.multiply(&rotation(&cross(from, to), clamp(dot(from, to), -1.0, 1.0).acos()))
}

/// gl-matrix's `quat.slerp`.
fn slerp(a: &Quaternion, b: &Quaternion, t: f64) -> Quaternion {
    let mut b = *b;
    let mut cos_omega = a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w;
    if cos_omega < 0.0 {
        cos_omega = -cos_omega;
        b = Quaternion::new(-b.x, -b.y, -b.z, -b.w);
    }
    let mut scale_a = 1.0 - t;
    let mut scale_b = t;
    if 1.0 - cos_omega > 1e-6 {
        let omega = cos_omega.acos();
        scale_a = ((1.0 - t) * omega).sin() / omega.sin();
        scale_b = (t * omega).sin() / omega.sin();
    }
    Quaternion::new(
        scale_a * a.x + scale_b * b.x,
        scale_a * a.y + scale_b * b.y,
        scale_a * a.z + scale_b * b.z,
        scale_a * a.w + scale_b * b.w,
    )
}

/// The point `t` of the way from one unit vector to another along the great
/// circle.
fn slerp_vec3(a: &Vec3, b: &Vec3, t: f64) -> Vec3 {
    let angle = clamp(dot(a, b), -1.0, 1.0).acos();
    if angle.sin() < 1e-9 {
        return *a;
    }
    add(&scaled(a, ((1.0 - t) * angle).sin() / angle.sin()), &scaled(b, (t * angle).sin() / angle.sin()))
}

/// The center and bearing of an orientation, the longitude unwrapped around
/// `center`.
fn center_and_bearing(orientation: &Quaternion, center: &LatLng) -> CenterAndBearing {
    let o = orientation_to_lat_lng_bearing(orientation);
    CenterAndBearing {
        center: lat_lng(
            o.latitude,
            center.longitude()
                + VerticalPerspectiveProjection::difference_of_angles_degrees(center.longitude(), o.longitude),
            WrapMode::Unwrapped,
        ),
        bearing: o.bearing,
    }
}

/// `zoom_around_point` with the bearing free. The exact part is the rotation
/// that brings the zoom location back under the anchor; the heuristic part
/// moves the center a `factor` of the way along the great circle towards it,
/// the orientation carried along. They blend by GL JS's horizon factor alone:
/// GL JS also blends by how far the location's longitude is from the
/// center's, which the bearing-held solution needs near a pole and a rotation
/// does not.
fn zoom_turning_the_planet(
    state: &mut TransformState,
    anchor: &ScreenCoordinate,
    zoom_location: &LatLng,
    factor: f64,
    interpolation_factor_horizon: f64,
) {
    let center = state.get_lat_lng(WrapMode::Unwrapped);
    let orientation = camera_orientation(state);
    let location = VerticalPerspectiveProjection::surface_vector(zoom_location);
    let center_vector = VerticalPerspectiveProjection::surface_vector(&center);
    let exact = turned(&orientation, &location, &pan_surface_vector(state, anchor));
    let heuristic = turned(&orientation, &slerp_vec3(&center_vector, &location, factor), &center_vector);
    let camera = center_and_bearing(
        &slerp(&exact, &heuristic, interpolation_factor_horizon.powf(INTERPOLATE_TO_HEURISTIC_EXPONENT)),
        &center,
    );
    let constrained = state.constrained_center(&camera.center, state.get_zoom());
    let zoom =
        state.get_zoom() + VerticalPerspectiveProjection::zoom_adjustment(center.latitude(), constrained.latitude());
    state.set_lat_lng_zoom(&constrained, zoom);
    state.set_bearing(-mln_math::deg2rad(camera.bearing));
}

impl ProjectionBase for VerticalPerspectiveProjection {
    fn project(&self, lat_lng: &LatLng, scale: f64) -> Point<f64> {
        Projection::project(lat_lng, scale)
    }

    fn unproject(&self, point: &Point<f64>, scale: f64, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        Projection::unproject(point, scale, wrap_mode)
    }

    fn tile_matrix(&self, matrix: &mut Mat4, tile_id: &UnwrappedTileID, scale: f64) {
        mercator_tile_matrix(matrix, tile_id, scale);
    }

    fn get_projection_data(
        &self,
        state: &TransformState,
        tile_id: &UnwrappedTileID,
        mercator_matrix: &Mat4,
    ) -> ProjectionData {
        ProjectionData {
            main_matrix: state.get_globe_view_projection_matrix(),
            tile_mercator_coords: mercator_tile_coords(tile_id),
            clipping_plane: state.get_globe_clipping_plane(),
            projection_transition: state.get_projection_transition(),
            fallback_matrix: *mercator_matrix,
            camera_position: state.get_globe_camera_position(),
            ..ProjectionData::default()
        }
    }

    fn project_tile_point(
        &self,
        data: &ProjectionData,
        tile_id: &UnwrappedTileID,
        point: &Point<f64>,
        elevation: f64,
    ) -> ProjectedTilePoint {
        let surface = Self::tile_coordinates_to_sphere(point, tile_id);
        let height_scale = 1.0 + elevation / Self::GLOBE_RADIUS_METERS;
        let sphere = scaled(&surface, height_scale);
        let pos = matrix::transform_mat4_value(&[sphere[0], sphere[1], sphere[2], 1.0], &data.main_matrix);
        // GL JS: the horizon plane hides a point on the surface, and only the
        // planet itself hides a raised one.
        let plane = &data.clipping_plane;
        let occluded = if height_scale <= 1.0 {
            plane[0] * surface[0] + plane[1] * surface[1] + plane[2] * surface[2] + plane[3] < 0.0
        } else {
            line_of_sight_blocked(&data.camera_position, &sphere)
        };
        ProjectedTilePoint {
            point: Point::new(pos[0] / pos[3], pos[1] / pos[3]),
            signed_distance_from_camera: pos[3],
            occluded,
        }
    }

    fn drawn_tile_point(
        &self,
        data: &ProjectionData,
        tile_id: &UnwrappedTileID,
        point: &Point<f64>,
        elevation: f64,
    ) -> ProjectedTilePoint {
        let globe = self.project_tile_point(data, tile_id, point, elevation);
        let t = data.projection_transition;
        if t > 0.999 {
            return globe;
        }
        // The shaders' `interpolateProjection`: the sphere blended with
        // Mercator, the horizon clipping through the blended depth from a
        // fifth of the hand-over on. Raised geometry keeps its depth and the
        // globe hides it.
        let flat = matrix::transform_mat4_value(&[point.x, point.y, elevation, 1.0], &data.fallback_matrix);
        let globe_w = globe.signed_distance_from_camera;
        let w = interpolate(&flat[3], &globe_w, t);
        let globeness = clamp((t - 0.2) / 0.8, 0.0, 1.0);
        let mut occluded = globeness > 0.0 && globe.occluded;
        if elevation <= 0.0 {
            let surface = Self::tile_coordinates_to_sphere(point, tile_id);
            let plane = &data.clipping_plane;
            let clipping_z = 1.0 - (plane[0] * surface[0] + plane[1] * surface[1] + plane[2] * surface[2] + plane[3]);
            occluded = globeness * clipping_z * globe_w > w;
        }
        ProjectedTilePoint {
            point: Point::new(
                interpolate(&flat[0], &(globe.point.x * globe_w), t) / w,
                interpolate(&flat[1], &(globe.point.y * globe_w), t) / w,
            ),
            signed_distance_from_camera: w,
            occluded,
        }
    }

    fn circle_radius_correction(&self, state: &TransformState) -> f64 {
        interpolate(
            &1.0,
            &Self::planet_scale(state.get_lat_lng(WrapMode::Unwrapped).latitude()),
            state.get_projection_transition(),
        )
    }

    fn pixel_scale(&self, state: &TransformState) -> f64 {
        interpolate(
            &1.0,
            &(1.0 / Self::planet_scale(state.get_lat_lng(WrapMode::Unwrapped).latitude())),
            state.get_projection_transition(),
        )
    }

    fn pitched_text_correction(
        &self,
        state: &TransformState,
        tile_anchor: &Point<f64>,
        tile_id: &UnwrappedTileID,
    ) -> f64 {
        let scale = 1.0 / (1u64 << tile_id.canonical.z) as f64;
        let mercator_y = tile_anchor.y / f64::from(EXTENT) * scale + f64::from(tile_id.canonical.y) * scale;
        let latitude = 2.0 * (PI - (mercator_y * PI * 2.0)).exp().atan() - PI * 0.5;
        interpolate(
            &1.0,
            &(Self::planet_scale(state.get_lat_lng(WrapMode::Unwrapped).latitude()) / latitude.cos()),
            state.get_projection_transition(),
        )
    }
}
