use std::f64::consts::PI;

use geometry::Point;
use glam::dcamera::rh::proj::opengl::perspective;
use glam::{DMat3, DMat4, DQuat, DVec3, DVec4, EulerRot, Vec3Swizzles, Vec4Swizzles};
use mln_style::constants::{EXTENT, LATITUDE_MAX, TILE_SIZE_D};
use mln_style::geo::{LatLng, WrapMode};
use mln_style::interpolate::interpolate;
use mln_tile_id::UnwrappedTileID;
use mln_util::geo::ScreenCoordinate;
use mln_util::projection::Projection;

use crate::projection::{ProjectedTilePoint, ProjectionData, mercator_tile_coords};
use crate::transform_state::TransformState;
use crate::view::{NorthOrientation, View};

/// How far from the polar axis a unit-sphere point can be and still be on its
/// pole, with no longitude of its own: GL JS
/// `sphereSurfacePointToCoordinates` gives it longitude 0.
const POLE_AXIS_DISTANCE: f64 = 1e-6;

fn rotate_x(v: DVec3, rad: f64) -> DVec3 {
    DMat3::from_rotation_x(rad) * v
}

fn rotate_y(v: DVec3, rad: f64) -> DVec3 {
    DMat3::from_rotation_y(rad) * v
}

fn rotate_z(v: DVec3, rad: f64) -> DVec3 {
    DMat3::from_rotation_z(rad) * v
}

/// GL JS `VerticalPerspectiveTransform._isLineOfSightBlocked`: whether the
/// planet stands between the camera and a point, both on the unit sphere's
/// scale.
fn line_of_sight_blocked(camera: DVec3, point: DVec3) -> bool {
    let to_point = point - camera;
    let length_sq = to_point.length_squared();
    if length_sq == 0.0 {
        return false;
    }
    let t = (-camera.dot(to_point) / length_sq).clamp(0.0, 1.0);
    (camera + to_point * t).length_squared() < 1.0
}

/// The distance along a ray to where it first meets the unit sphere at the
/// origin, by GL JS's numerically stable `raySphereIntersection`.
fn ray_sphere_intersection(origin: DVec3, direction: DVec3) -> Option<f64> {
    let origin_dot_direction = origin.dot(direction);
    let inner = origin - direction * origin_dot_direction;
    let discriminant = 1.0 - inner.length_squared();
    // A NaN ray, from a NaN point, hits nothing.
    if discriminant.is_nan() || discriminant < 0.0 {
        return None;
    }
    let c = origin.length_squared() - 1.0;
    let q = -origin_dot_direction + (if origin_dot_direction < 0.0 { 1.0 } else { -1.0 }) * discriminant.sqrt();
    let t0 = c / q;
    let t1 = q;
    // Both behind the ray's origin, as for a ray into the sky past a nearby
    // planet: no intersection.
    if f64::max(t0, t1) < 0.0 {
        return None;
    }
    Some(f64::min(t0, t1))
}

fn point_plane_signed_distance(plane: DVec4, point: DVec3) -> f64 {
    plane.dot(point.extend(1.0))
}

fn ray_direction_from_pixel(view: &View, point: &ScreenCoordinate, inverse_view_projection: &DMat4) -> DVec3 {
    let size = view.size();
    let clip =
        DVec3::new(point.x / f64::from(size.width) * 2.0 - 1.0, point.y / f64::from(size.height) * 2.0 - 1.0, 1.0);
    (inverse_view_projection.project_point3(clip) - view.globe_camera_position()).normalize()
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
    let angle = (x0 * x1 + y0 * y1).clamp(-1.0, 1.0).acos();
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
    let t = ((value - old_min) / (old_max - old_min)).clamp(0.0, 1.0);
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
    pub(crate) fn planet_scale(center_latitude: f64) -> f64 {
        mln_style::math::deg2rad(f64::min(center_latitude.abs(), LATITUDE_MAX)).cos()
    }

    /// The unit-sphere position of a point in tile units.
    #[must_use]
    pub fn tile_coordinates_to_sphere(tile_point: &Point<f64>, tile_id: &UnwrappedTileID) -> DVec3 {
        let [sin_x, cos_x, sin_y, cos_y] = Self::tile_coordinates_to_angles(tile_point, tile_id).to_array();
        DVec3::new(sin_x * cos_y, sin_y, cos_x * cos_y)
    }

    /// The sine and cosine of the longitude, then of the latitude, of a point
    /// in tile units. The longitude is 0 at the antimeridian, where Web
    /// Mercator's X is 0, so the prime meridian is +Z.
    fn tile_coordinates_to_angles(tile_point: &Point<f64>, tile_id: &UnwrappedTileID) -> DVec4 {
        let scale = 1.0 / (1u64 << tile_id.canonical.z) as f64;
        let mercator_x = tile_point.x / f64::from(EXTENT) * scale + f64::from(tile_id.canonical.x) * scale;
        let mercator_y = tile_point.y / f64::from(EXTENT) * scale + f64::from(tile_id.canonical.y) * scale;
        let spherical_x = mercator_x * PI * 2.0 + PI;
        // sin/cos of the latitude from the Mercator Y through the tangent
        // half-angle identities, as the shaders do.
        let t = (PI - (mercator_y * PI * 2.0)).exp();
        let t2 = t * t;
        DVec4::new(spherical_x.sin(), spherical_x.cos(), (t2 - 1.0) / (t2 + 1.0), (2.0 * t) / (t2 + 1.0))
    }

    /// The center's distance from the planet's center, in radii: the camera
    /// looks at the center raised by its altitude along its vertical, as the
    /// globe raises any height.
    fn center_radius(view: &View) -> f64 {
        1.0 + view.center_altitude() / Self::GLOBE_RADIUS_METERS
    }

    /// The view-projection matrix that takes unit-sphere positions to clip
    /// space. It draws the center, at its altitude, at the center of the
    /// padded view. Its near plane is half a pixel from the eye, or the
    /// terrain's ([`View::near_z`]) while terrain draws.
    #[must_use]
    pub fn globe_view_projection_matrix(view: &View, radius: f64) -> DMat4 {
        let size = view.size();
        let camera_to_center_distance = view.camera_to_center_distance();
        let center = view.center(WrapMode::Unwrapped);
        let near = if view.terrain_min_elevation().is_some() { view.near_z() } else { 0.5 };
        let center_radius = Self::center_radius(view);

        let mut projection = perspective(
            view.perspective_fov(),
            f64::from(size.width) / f64::from(size.height),
            near,
            camera_to_center_distance + radius * (center_radius + 1.0),
        );
        // Move the vanishing point to the center of the padded view.
        let offset = view.center_offset();
        projection.z_axis.x = -offset.x * 2.0 / f64::from(size.width);
        projection.z_axis.y = offset.y * 2.0 / f64::from(size.height);

        if view.north_orientation() != NorthOrientation::Upwards {
            projection *= DMat4::from_rotation_z(-view.north_orientation_angle());
        }

        projection
            * DMat4::from_translation(DVec3::new(0.0, 0.0, -camera_to_center_distance))
            * DMat4::from_rotation_z(view.roll())
            * DMat4::from_rotation_x(-view.pitch())
            * DMat4::from_rotation_z(-view.angle())
            * DMat4::from_translation(DVec3::new(0.0, 0.0, -radius * center_radius))
            * DMat4::from_rotation_x(mln_style::math::deg2rad(center.latitude()))
            * DMat4::from_rotation_y(-mln_style::math::deg2rad(center.longitude()))
            * DMat4::from_scale(DVec3::splat(radius))
    }

    /// The plane (as ax + by + cz + d) that separates the part of the
    /// sea-level sphere the camera sees from the back of the globe.
    #[must_use]
    pub fn clipping_plane(view: &View, radius: f64) -> DVec4 {
        let pitch = view.pitch();
        let distance_camera_to_b = view.camera_to_center_distance() / radius;
        let distance_camera_to_a = pitch.sin() * distance_camera_to_b;
        let distance_a_to_c = pitch.cos() * distance_camera_to_b + Self::center_radius(view);
        let distance_camera_to_c =
            (distance_camera_to_a * distance_camera_to_a + distance_a_to_c * distance_a_to_c).sqrt();
        let tangent_plane_distance_to_c = 1.0 / distance_camera_to_c;

        let mut plane =
            DVec3::new(0.0, -distance_camera_to_a / distance_camera_to_c, distance_a_to_c / distance_camera_to_c);

        let center = view.center(WrapMode::Unwrapped);
        plane = rotate_z(plane, view.angle());
        plane = rotate_x(plane, -mln_style::math::deg2rad(center.latitude()));
        plane = rotate_y(plane, mln_style::math::deg2rad(center.longitude()));

        plane.extend(-tangent_plane_distance_to_c) / plane.length()
    }

    /// Camera position in unit-sphere space, back from the center at its
    /// altitude.
    #[must_use]
    pub fn camera_position(view: &View, radius: f64) -> DVec3 {
        let center = view.center(WrapMode::Unwrapped);
        let mut position = DVec3::new(0.0, 0.0, view.camera_to_center_distance() / radius);
        position = rotate_z(position, -view.roll());
        position = rotate_x(position, view.pitch());
        position = rotate_z(position, view.angle());
        position += DVec3::Z * Self::center_radius(view);
        position = rotate_x(position, -mln_style::math::deg2rad(center.latitude()));
        rotate_y(position, mln_style::math::deg2rad(center.longitude()))
    }

    #[must_use]
    pub fn surface_vector(lat_lng: &LatLng) -> DVec3 {
        let lng = mln_style::math::deg2rad(lat_lng.longitude());
        let lat = mln_style::math::deg2rad(lat_lng.latitude());
        let len = lat.cos();
        DVec3::new(lng.sin() * len, lat.sin(), lng.cos() * len)
    }

    /// The location of a unit-sphere point, or `None` for a point that isn't
    /// a number, as a degenerate camera's can be.
    pub(crate) fn surface_vector_to_lat_lng(surface: DVec3) -> Option<LatLng> {
        let latitude = mln_style::math::rad2deg(surface.y.clamp(-1.0, 1.0).asin());
        let length_xz = surface.xz().length();
        if length_xz <= POLE_AXIS_DISTANCE {
            return LatLng::with_mode(latitude, 0.0, WrapMode::Unwrapped).ok();
        }
        let acos_z = (surface.z / length_xz).clamp(-1.0, 1.0).acos();
        let longitude = mln_style::math::rad2deg(if surface.x > 0.0 { acos_z } else { -acos_z });
        LatLng::with_mode(latitude, longitude, WrapMode::Wrapped).ok()
    }

    /// The Web Mercator point at zoom 0 under the unit-sphere point `point`,
    /// in the world copy nearest the view's center, and its height in meters
    /// above the sea-level sphere; `None` for a point that isn't a number.
    /// Over the polar caps, past the latitudes that Web Mercator's square
    /// holds, y is past 0 or 1, and at the poles infinite or far past.
    pub(crate) fn sphere_point_to_world(view: &View, point: DVec3) -> Option<DVec3> {
        let length = point.length();
        let location = view.lat_lng_near_center(&Self::surface_vector_to_lat_lng(point / length)?);
        let world = Projection::project(&location, 1.0 / TILE_SIZE_D);
        let latitude = location.latitude();
        // `Projection::project`'s formula, without its clamp.
        let y = if latitude.abs() > LATITUDE_MAX {
            (180.0 - mln_style::math::rad2deg((PI / 4.0 + mln_style::math::deg2rad(latitude) / 2.0).tan().ln())) / 360.0
        } else {
            world.y
        };
        Some(DVec3::new(world.x, y, (length - 1.0) * Self::GLOBE_RADIUS_METERS))
    }

    /// The direction from the planet's center, as a unit-sphere point, of
    /// where the ray through a screen pixel (y up, as `TransformState` takes
    /// it) meets the sphere `height` meters above sea level; pixels off that
    /// sphere snap to the nearest point on its horizon. It solves on the unit
    /// sphere with the camera scaled by 1 / (1 + height / R), which at
    /// height 0 is the sea-level sphere's math to the bit.
    pub(crate) fn screen_coordinate_to_surface(view: &View, point: &ScreenCoordinate, height: f64) -> DVec3 {
        let origin = camera_over_sphere(view, height);
        let direction = ray_direction_from_pixel(view, point, &view.inverse_globe_view_projection_matrix());

        if let Some(t) = ray_sphere_intersection(origin, direction) {
            return (origin + direction * t).normalize();
        }

        // The ray misses the sphere: take the nearest point on the horizon,
        // the circle where the clipping plane cuts it.
        let plane = if height == 0.0 {
            view.globe_clipping_plane()
        } else {
            let distance = origin.length();
            (origin / distance).extend(-1.0 / distance)
        };
        let normal = plane.xyz();
        let distance_to_intersection = -point_plane_signed_distance(plane, origin) / direction.dot(normal);

        let plane_intersection = if distance_to_intersection > 0.0 {
            origin + direction * distance_to_intersection
        } else {
            const MAX_RAY_LENGTH: f64 = 2.0;
            let distant_point = origin + direction * MAX_RAY_LENGTH;
            distant_point - normal * point_plane_signed_distance(plane, distant_point)
        };

        let horizon_center = normal * -plane.w;
        let horizon_radius = f64::max(0.0, 1.0 - plane.w * plane.w).sqrt();
        let relative = plane_intersection - horizon_center;
        horizon_center + relative * (horizon_radius / relative.length())
    }

    /// Whether a screen pixel (y up) looks at the sphere `height` meters
    /// above sea level rather than past it, from a camera outside it.
    #[must_use]
    pub fn screen_coordinate_hits_globe(view: &View, point: &ScreenCoordinate, height: f64) -> bool {
        let direction = ray_direction_from_pixel(view, point, &view.inverse_globe_view_projection_matrix());
        ray_sphere_intersection(camera_over_sphere(view, height), direction).is_some()
    }

    /// The location under a screen pixel (y up) on the sphere `height`
    /// meters above sea level ([`Self::screen_coordinate_to_surface`]).
    pub(crate) fn screen_coordinate_to_lat_lng(
        view: &View,
        point: &ScreenCoordinate,
        height: f64,
        wrap_mode: WrapMode,
    ) -> Option<LatLng> {
        let lat_lng = Self::surface_vector_to_lat_lng(Self::screen_coordinate_to_surface(view, point, height))?;
        Some(if wrap_mode == WrapMode::Wrapped { lat_lng.wrapped() } else { lat_lng })
    }

    /// The screen point (y up) of a location `height` meters above sea level
    /// on the globe, raised along its vertical, and its clip-space position.
    #[must_use]
    pub fn lat_lng_to_screen_coordinate(view: &View, lat_lng: &LatLng, height: f64) -> (ScreenCoordinate, DVec4) {
        let sphere = Self::surface_vector(lat_lng) * (1.0 + height / Self::GLOBE_RADIUS_METERS);
        let clip = view.globe_view_projection_matrix() * sphere.extend(1.0);
        let size = view.size();
        let point = Point::new(
            (clip.x / clip.w * 0.5 + 0.5) * f64::from(size.width),
            (clip.y / clip.w * 0.5 + 0.5) * f64::from(size.height),
        );
        (point, clip)
    }

    /// The center that puts `lat_lng`, on the sphere `height` meters above
    /// sea level, under `anchor` with the bearing unchanged, if one exists.
    pub(crate) fn center_for_location_at_point(
        view: &View,
        lat_lng: &LatLng,
        anchor: &ScreenCoordinate,
        height: f64,
    ) -> Option<LatLng> {
        // GL JS `setLocationAtPoint`: solve the rotation that carries the
        // surface point under the pixel onto the target. Turning the planet
        // carries every sphere about its center with it, so the solve takes
        // the direction of the point under the pixel on the sphere at
        // `height`.
        const LIMIT: f64 = PI * 0.5;
        let center = view.center(WrapMode::Unwrapped);
        let vec_to_pixel_current = Self::screen_coordinate_to_surface(view, anchor, height);
        let vec_to_target = Self::surface_vector(lat_lng);

        let rotated_pixel_vector = rotate_x(
            rotate_y(vec_to_pixel_current, -mln_style::math::deg2rad(center.longitude())),
            mln_style::math::deg2rad(center.latitude()),
        );

        let vec_to_target_xz_length_squared = vec_to_target.xz().length_squared();
        let target_x_squared = rotated_pixel_vector.x * rotated_pixel_vector.x;
        if vec_to_target_xz_length_squared < target_x_squared {
            return None;
        }

        let intersection_a = (vec_to_target_xz_length_squared - target_x_squared).sqrt();
        let intersection_b = -intersection_a;

        let solve = |intersection: f64| {
            let lng = angle_to_rotate_between_vectors_2d(
                vec_to_target.x,
                vec_to_target.z,
                rotated_pixel_vector.x,
                intersection,
            );
            let vec_to_target_lng = rotate_y(vec_to_target, -lng);
            let lat = angle_to_rotate_between_vectors_2d(
                vec_to_target_lng.y,
                vec_to_target_lng.z,
                rotated_pixel_vector.y,
                rotated_pixel_vector.z,
            );
            (lng, lat)
        };
        let (lng_a, lat_a) = solve(intersection_a);
        let (lng_b, lat_b) = solve(intersection_b);

        let valid_a = (-LIMIT..=LIMIT).contains(&lat_a);
        let valid_b = (-LIMIT..=LIMIT).contains(&lat_b);

        let (lng, lat) = if valid_a && valid_b {
            let center_lng = mln_style::math::deg2rad(center.longitude());
            let center_lat = mln_style::math::deg2rad(center.latitude());
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
        Some(LatLng::saturating(
            (mln_style::math::rad2deg(lat)).clamp(-90.0, 90.0),
            mln_style::math::rad2deg(lng),
            WrapMode::Unwrapped,
        ))
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
    /// it. `zoom_location` stands on the sphere `height` meters above sea
    /// level, which the exact part solves on.
    pub fn zoom_around_point(
        state: &mut TransformState,
        anchor: &ScreenCoordinate,
        zoom_location: &LatLng,
        height: f64,
        zoom_delta: f64,
        free_bearing: bool,
    ) {
        if zoom_delta == 0.0 {
            return;
        }
        let center = state.view().center(WrapMode::Unwrapped);
        // A location on a pole has no longitude of its own, so the zoom keeps
        // the center's meridian rather than turning the planet about the pole
        // towards the longitude the location was given.
        let on_pole = mln_style::math::deg2rad(zoom_location.latitude()).cos() <= POLE_AXIS_DISTANCE;
        let d_lng_raw = if on_pole {
            0.0
        } else {
            Self::difference_of_angles_degrees(center.longitude(), zoom_location.longitude())
        };
        let d_lng = d_lng_raw / ((d_lng_raw / 180.0).abs() + 1.0);
        let d_lat = Self::difference_of_angles_degrees(center.latitude(), zoom_location.latitude());

        // Slow the movement down when the anchor's ray passes far from the
        // planet.
        let ray_origin = camera_over_sphere(state.view(), height);
        let ray_direction =
            ray_direction_from_pixel(state.view(), anchor, &state.view().inverse_globe_view_projection_matrix());
        let closest_point = ray_origin - ray_direction * ray_origin.dot(ray_direction);
        let ray_distance_from_globe_center = closest_point.length();
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
        let size = state.view().size();
        let radius = Self::globe_radius_pixels(Projection::world_size(state.view().scale()), center.latitude())
            / f64::from(size.width.min(size.height));
        let radius_factor =
            remap_saturate(radius, SLOWING_RADIUS_START, SLOWING_RADIUS_STOP, 1.0, SLOWING_RADIUS_SLOW_FACTOR);
        let slowing_factor = f64::min(distance_factor, 1.0 + (radius_factor - 1.0) * interpolation_factor_horizon);

        let factor = (1.0 - 2.0f64.powf(-zoom_delta)) * slowing_factor;
        if free_bearing {
            zoom_turning_the_planet(state, anchor, zoom_location, height, factor, interpolation_factor_horizon);
            return;
        }
        let heuristic_center = LatLng::saturating(
            (center.latitude() + d_lat * factor).clamp(-90.0, 90.0),
            center.longitude() + d_lng * factor,
            WrapMode::Unwrapped,
        );

        let mut exact_center =
            Self::center_for_location_at_point(state.view(), zoom_location, anchor, height).unwrap_or(center);
        if on_pole {
            exact_center = LatLng::saturating(exact_center.latitude(), center.longitude(), WrapMode::Unwrapped);
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
        let target = LatLng::saturating(
            (exact_center.latitude()
                + Self::difference_of_angles_degrees(exact_center.latitude(), heuristic_center.latitude())
                    * heuristic_factor)
                .clamp(-90.0, 90.0),
            exact_center.longitude()
                + Self::difference_of_angles_degrees(exact_center.longitude(), heuristic_center.longitude())
                    * heuristic_factor,
            WrapMode::Unwrapped,
        );
        let constrained = state.constrained_center(&target, state.view().zoom());
        let zoom = state.view().zoom() + Self::zoom_adjustment(center.latitude(), constrained.latitude());
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
    pub(crate) fn max_latitude(view: &View, zoom: f64) -> f64 {
        let area_radians = mln_style::math::deg2rad(90.0 - LATITUDE_MAX);
        let area_pixels =
            Self::globe_radius_pixels(Projection::world_size(2.0f64.powf(zoom)), LATITUDE_MAX) * area_radians;
        let size = view.size();
        let reach_pixels = f64::from(size.width.min(size.height)) / 2.0;
        let center_pixels = f64::min(area_pixels, reach_pixels)
            * remap_saturate(area_pixels, reach_pixels, reach_pixels * POLE_AREA_FADE_SCALE, 1.0, 0.0);
        LATITUDE_MAX + mln_style::math::rad2deg(area_radians * center_pixels / area_pixels)
    }

    /// GL JS `computeGlobePanCenter`: the center after a pan of `offset`
    /// pixels, turning the planet by degrees per pixel at the center's
    /// latitude. The longitude is unwrapped and the latitude clamped to the
    /// poles.
    #[must_use]
    pub fn pan_center(view: &View, offset: &ScreenCoordinate) -> LatLng {
        // GL JS turns the pan by its bearing, which is the view's angle with the
        // sign flipped.
        let bearing = -view.angle();
        let x = offset.x * bearing.cos() - offset.y * bearing.sin();
        let y = offset.x * bearing.sin() + offset.y * bearing.cos();

        let center = view.center(WrapMode::Unwrapped);
        let latitude = center.latitude();
        let inverse_scale = |lat: f64| 1.0 / Self::planet_scale(lat);
        // Longitude speeds up towards the poles, where the planet is
        // narrower, but as the globe zooms out from 7 to 3 the latitude it
        // speeds up for is capped at 60°: GL JS's numbers.
        let zoomed_out = ((7.0 - (view.zoom() + Self::zoom_adjustment(latitude, 0.0))) / 4.0).clamp(0.0, 1.0);
        let longitude_speed =
            interpolate(&inverse_scale(latitude), &inverse_scale(f64::min(latitude.abs(), 60.0)), zoomed_out);
        let degrees_per_pixel =
            360.0 / (2.0 * PI * Self::globe_radius_pixels(Projection::world_size(view.scale()), latitude));
        LatLng::saturating(
            (latitude + y * degrees_per_pixel).clamp(-90.0, 90.0),
            center.longitude() - x * degrees_per_pixel * longitude_speed,
            WrapMode::Unwrapped,
        )
    }

    /// GL JS `versorSetLocationAtPoint` without the fixed bearing: the whole
    /// rotation that brings the location under `point - pan_delta` under
    /// `point` (both y up), so the grabbed location tracks the cursor exactly
    /// and the bearing turns with the planet, over the poles too. The
    /// location is on the sphere `height` meters above sea level. The
    /// longitude stays unwrapped around the current center, the latitude is
    /// not yet constrained.
    #[must_use]
    pub fn drag_camera(
        view: &View,
        point: &ScreenCoordinate,
        pan_delta: &ScreenCoordinate,
        height: f64,
    ) -> CenterAndBearing {
        center_and_bearing(
            turned(
                camera_orientation(view),
                pan_surface_vector(view, &Point::new(point.x - pan_delta.x, point.y - pan_delta.y), height),
                pan_surface_vector(view, point, height),
            ),
            &view.center(WrapMode::Unwrapped),
        )
    }

    /// Where a fling of `offset` pixels (y up) ends with the bearing free:
    /// the planet turns about the axis a drag in that direction at `center`
    /// (y up) turns it about, by GL JS's degrees per pixel at the center's
    /// latitude and short of half a turn, the orientation carried along. Its
    /// axis is the sea-level sphere's, as its pace is.
    #[must_use]
    pub fn fling_camera(view: &View, center: &ScreenCoordinate, offset: &ScreenCoordinate) -> CenterAndBearing {
        let lat_lng = view.center(WrapMode::Unwrapped);
        let pixels = offset.x.hypot(offset.y);
        if pixels == 0.0 {
            return CenterAndBearing { center: lat_lng, bearing: -mln_style::math::rad2deg(view.angle()) };
        }
        let axis =
            pan_surface_vector(view, &Point::new(center.x - offset.x / pixels, center.y - offset.y / pixels), 0.0)
                .cross(pan_surface_vector(view, center, 0.0));
        let degrees_per_pixel =
            360.0 / (2.0 * PI * Self::globe_radius_pixels(Projection::world_size(view.scale()), lat_lng.latitude()));
        center_and_bearing(
            camera_orientation(view)
                * rotation(axis, mln_style::math::deg2rad(f64::min(pixels * degrees_per_pixel, 179.5))),
            &lat_lng,
        )
    }

    /// The camera `t` of the way from one camera to another, the planet
    /// turning about one axis.
    #[must_use]
    pub fn interpolate_camera(from: &CenterAndBearing, to: &CenterAndBearing, t: f64) -> CenterAndBearing {
        center_and_bearing(
            orientation_from_lat_lng_bearing(&from.center, from.bearing)
                .slerp(orientation_from_lat_lng_bearing(&to.center, to.bearing), t),
            &from.center,
        )
    }

    /// GL JS `versorSetLocationAtPoint` with the bearing held, as a drag uses
    /// it: the center after the one rotation that brings the location under
    /// `point - pan_delta` back under `point` (both y up), on the sphere
    /// `height` meters above sea level. Within 12 degrees of the pole's edge
    /// the cursor turns a dial around the pole, and a swing over a pole stops
    /// on it. The longitude stays unwrapped around the current center, the
    /// latitude is not yet constrained.
    #[must_use]
    pub fn drag_center(view: &View, point: &ScreenCoordinate, pan_delta: &ScreenCoordinate, height: f64) -> LatLng {
        let center = view.center(WrapMode::Unwrapped);
        // GL JS's bearing is the view's angle with the sign flipped.
        let bearing = -mln_style::math::rad2deg(view.angle());
        let swung = orientation_to_lat_lng_bearing(turned(
            camera_orientation(view),
            pan_surface_vector(view, &Point::new(point.x - pan_delta.x, point.y - pan_delta.y), height),
            pan_surface_vector(view, point, height),
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
        let (pole, _) = Self::lat_lng_to_screen_coordinate(
            view,
            &LatLng::saturating(pole_latitude, 0.0, WrapMode::Unwrapped),
            height,
        );
        let rx = point.x - pole.x;
        let ry = pole.y - point.y;
        let ramp = (1.0 - (LATITUDE_MAX - center.latitude().abs()) / DIAL_RAMP_DEGREES).clamp(0.0, 1.0);
        let dial = ramp * ramp * (3.0 - 2.0 * ramp);
        let d_lng_swing = mln_style::math::wrap(swung.longitude - center.longitude() + 180.0, 0.0, 360.0) - 180.0;
        let mut d_lng_dial = 0.0;
        if dial > 0.0 {
            let radius_squared = f64::max(rx * rx + ry * ry, DIAL_MIN_RADIUS_PIXELS * DIAL_MIN_RADIUS_PIXELS);
            let d_theta = (rx * -pan_delta.y - ry * pan_delta.x) / radius_squared;
            d_lng_dial = (if pole_latitude > 0.0 { 1.0 } else { -1.0 }) * mln_style::math::rad2deg(d_theta);
        }
        LatLng::saturating(
            latitude,
            center.longitude() + (1.0 - dial) * d_lng_swing + dial * d_lng_dial,
            WrapMode::Unwrapped,
        )
    }

    /// The center that puts `location`, on the sphere `height` meters above
    /// sea level, under `point` (y up) with the bearing held, where a drag
    /// can hold a location exactly. Elsewhere `None`, and
    /// [`Self::drag_center`] makes the step:
    ///
    /// - `point` is off the planet, or in the band at its edge where the
    ///   drag eases off;
    /// - the center is where the cursor turns the dial around a pole;
    /// - no center puts `location` there, or the one that does turns more
    ///   than `DRAG_HOLD_MAX_GAIN` times as far as the cursor moves, as for
    ///   a location near a pole or a quarter turn of longitude from the
    ///   center.
    ///
    /// The planet keeps its size on screen as the center changes latitude
    /// ([`Self::zoom_adjustment`]), so the center holds `location` at the
    /// adjusted zoom. The longitude stays unwrapped around the current
    /// center, the latitude is not yet constrained.
    #[must_use]
    pub fn drag_center_holding(
        view: &View,
        location: &LatLng,
        point: &ScreenCoordinate,
        height: f64,
    ) -> Option<LatLng> {
        let center = view.center(WrapMode::Unwrapped);
        if center.latitude().abs() > LATITUDE_MAX - DIAL_RAMP_DEGREES
            || !Self::screen_coordinate_hits_globe(view, point, height)
            || pan_falloff(view, point, height).is_some()
        {
            return None;
        }
        let held = Self::center_for_location_at_point(view, location, point, height)?;
        // A pixel of cursor travel turns the center 1 / (cos(latitude) *
        // cos(longitude from the center)) times as far as it does for a
        // location at the center.
        let gain = mln_style::math::deg2rad(location.latitude()).cos()
            * mln_style::math::deg2rad(location.longitude() - held.longitude()).cos();
        if gain.abs() * DRAG_HOLD_MAX_GAIN < 1.0 {
            return None;
        }
        Some(LatLng::saturating(
            held.latitude(),
            center.longitude() + Self::difference_of_angles_degrees(center.longitude(), held.longitude()),
            WrapMode::Unwrapped,
        ))
    }

    /// The shorter signed way around from one angle to the other, in degrees.
    #[must_use]
    pub fn difference_of_angles_degrees(from: f64, to: f64) -> f64 {
        let a = mln_style::math::wrap(from, 0.0, 360.0);
        let b = mln_style::math::wrap(to, 0.0, 360.0);
        let direct = b - a;
        let around = if b > a { direct - 360.0 } else { direct + 360.0 };
        if direct.abs() < around.abs() { direct } else { around }
    }

    /// Distance between two locations along the surface, in pixels at the
    /// given world size and center latitude.
    #[must_use]
    pub fn surface_distance_pixels(world_size: f64, center_latitude: f64, a: &LatLng, b: &LatLng) -> f64 {
        let radians = Self::surface_vector(a).dot(Self::surface_vector(b)).clamp(-1.0, 1.0).acos();
        radians * Self::globe_radius_pixels(world_size, center_latitude)
    }

    /// The location `t` of the way from `start` to `start + delta`, the
    /// longitude paced at 1 / cos(latitude) so the globe appears to turn at a
    /// steady rate. The pace grows without bound towards the poles, so past
    /// the Mercator range it keeps its value at the edge.
    #[must_use]
    pub fn interpolate_lat_lng(start: &LatLng, delta_latitude: f64, delta_longitude: f64, t: f64) -> LatLng {
        let latitude = (start.latitude() + delta_latitude * t).clamp(-90.0, 90.0);
        let start_latitude = start.latitude().clamp(-LATITUDE_MAX, LATITUDE_MAX);
        let end_latitude = (start.latitude() + delta_latitude).clamp(-LATITUDE_MAX, LATITUDE_MAX);
        if (end_latitude - start_latitude).abs() <= 1.0 {
            return LatLng::saturating(latitude, start.longitude() + delta_longitude * t, WrapMode::Unwrapped);
        }
        let on_different_hemispheres = sign(end_latitude) != sign(start_latitude);
        let sample_start = mln_style::math::deg2rad(if on_different_hemispheres {
            -start_latitude.abs()
        } else {
            start_latitude.abs()
        });
        let sample_end = mln_style::math::deg2rad(end_latitude.abs());
        let value_start = integrate_secant(sample_start);
        let value_end = integrate_secant(sample_end);
        let value_t = integrate_secant(sample_start + t * (sample_end - sample_start));
        let longitude_t = (value_t - value_start) / (value_end - value_start);
        LatLng::saturating(latitude, start.longitude() + delta_longitude * longitude_t, WrapMode::Unwrapped)
    }
}

// GL JS globe_utils drag constants.
const PAN_FALLOFF_BAND: f64 = 0.1;
const DIAL_MIN_RADIUS_PIXELS: f64 = 20.0;
const DIAL_RAMP_DEGREES: f64 = 12.0;
const PAN_MAX_ANGLE: f64 = PI * 0.98;

/// The most a held location may turn the center per pixel, relative to a
/// location at the center, before the drag stops holding it exactly.
const DRAG_HOLD_MAX_GAIN: f64 = 2.5;

/// GL JS `orientationFromLngLatBearing`: the rotation by `-longitude` about
/// x, then `-latitude` about y, then the bearing (in GL JS's sign) about z.
fn orientation_from_lat_lng_bearing(center: &LatLng, bearing_degrees: f64) -> DQuat {
    DQuat::from_euler(
        EulerRot::ZYX,
        mln_style::math::deg2rad(bearing_degrees),
        mln_style::math::deg2rad(-center.latitude()),
        mln_style::math::deg2rad(-center.longitude()),
    )
}

struct Orientation {
    latitude: f64,
    longitude: f64,
    bearing: f64,
}

/// GL JS `lngLatBearingFromOrientation`.
fn orientation_to_lat_lng_bearing(q: DQuat) -> Orientation {
    Orientation {
        latitude: -mln_style::math::rad2deg((2.0 * (q.w * q.y - q.z * q.x)).clamp(-1.0, 1.0).asin()),
        longitude: -mln_style::math::rad2deg(
            (2.0 * (q.w * q.x + q.y * q.z)).atan2(1.0 - 2.0 * (q.x * q.x + q.y * q.y)),
        ),
        bearing: mln_style::math::rad2deg((2.0 * (q.w * q.z + q.x * q.y)).atan2(1.0 - 2.0 * (q.y * q.y + q.z * q.z))),
    }
}

/// A drag's ray through a point in the falloff band of
/// [`pan_surface_vector`], or past it.
struct PanFalloff {
    /// The direction from the planet's center to the camera.
    toward_camera: DVec3,
    /// The camera's distance from the planet's center, in radii of the
    /// sphere the drag holds.
    distance: f64,
    /// The part of the ray's direction across `toward_camera`, and its
    /// length.
    lateral: DVec3,
    lateral_length: f64,
    /// The ray's angle from the line to the planet's center, and the angle
    /// the band starts at.
    angle: f64,
    handover_angle: f64,
}

/// The falloff a drag towards `point` on the sphere `height` meters above sea
/// level is in, if any: none while the ray is well inside that sphere's
/// silhouette, where the drag aims at the point under `point`.
fn pan_falloff(view: &View, point: &ScreenCoordinate, height: f64) -> Option<PanFalloff> {
    let origin = camera_over_sphere(view, height);
    let distance = origin.length();
    if distance <= 1.0 {
        return None;
    }
    let toward_camera = origin / distance;
    let direction = ray_direction_from_pixel(view, point, &view.inverse_globe_view_projection_matrix());
    let c = -direction.dot(toward_camera);
    let lateral = direction + toward_camera * c;
    let lateral_length = lateral.length();
    if lateral_length < 1e-9 {
        return None;
    }
    let angle = lateral_length.atan2(c);
    let handover_angle = (1.0 / distance).asin() * (1.0 - PAN_FALLOFF_BAND);
    (angle >= handover_angle).then_some(PanFalloff {
        toward_camera,
        distance,
        lateral,
        lateral_length,
        angle,
        handover_angle,
    })
}

/// GL JS `panSurfaceLocation`: the direction of the point on the sphere
/// `height` meters above sea level that a drag towards `point` aims at. Exact
/// while the ray is well inside the silhouette, then eased off along a
/// hyperbola that matches it in value and slope and saturates short of the
/// far side, so the drag keeps going once the cursor leaves the planet.
fn pan_surface_vector(view: &View, point: &ScreenCoordinate, height: f64) -> DVec3 {
    let Some(PanFalloff { toward_camera: u, distance, lateral, lateral_length: s, angle, handover_angle }) =
        pan_falloff(view, point, height)
    else {
        return VerticalPerspectiveProjection::screen_coordinate_to_surface(view, point, height);
    };
    let sin_handover = distance * handover_angle.sin();
    let target_at_handover = sin_handover.clamp(-1.0, 1.0).asin() - handover_angle;
    let slope_at_handover =
        distance * handover_angle.cos() / f64::max(1.0 - sin_handover * sin_handover, 1e-12).sqrt() - 1.0;
    let room = PAN_MAX_ANGLE - target_at_handover;
    let excess = angle - handover_angle;
    let target = (target_at_handover + room * (slope_at_handover * excess) / (room + slope_at_handover * excess))
        .clamp(0.0, PAN_MAX_ANGLE);
    (u * target.cos() + lateral * (target.sin() / s)).normalize()
}

/// The globe camera's position where the sphere `height` meters above sea
/// level is the unit sphere: scaled by 1 / (1 + height / R), which at height
/// 0 leaves it as it is, to the bit.
fn camera_over_sphere(view: &View, height: f64) -> DVec3 {
    view.globe_camera_position() / (1.0 + height / VerticalPerspectiveProjection::GLOBE_RADIUS_METERS)
}

/// The globe camera's orientation; GL JS's bearing is the view's angle with the
/// sign flipped.
fn camera_orientation(view: &View) -> DQuat {
    orientation_from_lat_lng_bearing(&view.center(WrapMode::Unwrapped), -mln_style::math::rad2deg(view.angle()))
}

/// The rotation about `axis` by `angle`, composed as GL JS's drag composes
/// it: the axis is in the surface-vector frame and the orientation in the
/// Euler frame, hence the swizzle.
fn rotation(axis: DVec3, angle: f64) -> DQuat {
    let l = axis.length();
    if l == 0.0 {
        return DQuat::IDENTITY;
    }
    let s = (angle / 2.0).sin();
    DQuat::from_xyzw(axis.y / l * s, -axis.x / l * s, axis.z / l * s, (angle / 2.0).cos())
}

/// The orientation after the rotation that brings the surface location
/// `from` to where `to` is.
fn turned(orientation: DQuat, from: DVec3, to: DVec3) -> DQuat {
    orientation * rotation(from.cross(to), from.dot(to).clamp(-1.0, 1.0).acos())
}

/// The point `t` of the way from one unit vector to another along the great
/// circle.
fn slerp_vec3(a: DVec3, b: DVec3, t: f64) -> DVec3 {
    let angle = a.dot(b).clamp(-1.0, 1.0).acos();
    if angle.sin() < 1e-9 {
        return a;
    }
    a * (((1.0 - t) * angle).sin() / angle.sin()) + b * ((t * angle).sin() / angle.sin())
}

/// The center and bearing of an orientation, the longitude unwrapped around
/// `center`.
fn center_and_bearing(orientation: DQuat, center: &LatLng) -> CenterAndBearing {
    let o = orientation_to_lat_lng_bearing(orientation);
    CenterAndBearing {
        center: LatLng::saturating(
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
    height: f64,
    factor: f64,
    interpolation_factor_horizon: f64,
) {
    let center = state.view().center(WrapMode::Unwrapped);
    let orientation = camera_orientation(state.view());
    let location = VerticalPerspectiveProjection::surface_vector(zoom_location);
    let center_vector = VerticalPerspectiveProjection::surface_vector(&center);
    let exact = turned(orientation, location, pan_surface_vector(state.view(), anchor, height));
    let heuristic = turned(orientation, slerp_vec3(center_vector, location, factor), center_vector);
    let camera = center_and_bearing(
        exact.slerp(heuristic, interpolation_factor_horizon.powf(INTERPOLATE_TO_HEURISTIC_EXPONENT)),
        &center,
    );
    let constrained = state.constrained_center(&camera.center, state.view().zoom());
    let zoom =
        state.view().zoom() + VerticalPerspectiveProjection::zoom_adjustment(center.latitude(), constrained.latitude());
    state.set_lat_lng_zoom(&constrained, zoom);
    state.set_angle(-mln_style::math::deg2rad(camera.bearing));
}

impl VerticalPerspectiveProjection {
    /// [`Projection::projection_data`](crate::Projection::projection_data) on the globe.
    pub(crate) fn projection_data(view: &View, tile_id: &UnwrappedTileID, mercator_matrix: &DMat4) -> ProjectionData {
        // The middle of the tile; the shaders' `GLOBE_ANCHOR`.
        let middle = Point::new(f64::from(EXTENT) / 2.0, f64::from(EXTENT) / 2.0);
        ProjectionData {
            sphere_anchor: Self::tile_coordinates_to_sphere(&middle, tile_id),
            sphere_anchor_angles: Self::tile_coordinates_to_angles(&middle, tile_id),
            main_matrix: view.globe_view_projection_matrix(),
            tile_mercator_coords: mercator_tile_coords(tile_id),
            clipping_plane: view.globe_clipping_plane(),
            projection_transition: view.projection_transition(),
            fallback_matrix: *mercator_matrix,
            camera_position: view.globe_camera_position(),
            ..ProjectionData::default()
        }
    }

    /// [`Projection::project_tile_point`](crate::Projection::project_tile_point) on the globe.
    pub(crate) fn project_tile_point(
        data: &ProjectionData,
        tile_id: &UnwrappedTileID,
        point: &Point<f64>,
        elevation: f64,
    ) -> ProjectedTilePoint {
        let surface = Self::tile_coordinates_to_sphere(point, tile_id);
        let height_scale = 1.0 + elevation / Self::GLOBE_RADIUS_METERS;
        let sphere = surface * height_scale;
        let pos = data.main_matrix * sphere.extend(1.0);
        // GL JS: the horizon plane hides a point on the surface, and only the
        // planet itself hides a raised one.
        let occluded = if height_scale <= 1.0 {
            point_plane_signed_distance(data.clipping_plane, surface) < 0.0
        } else {
            line_of_sight_blocked(data.camera_position, sphere)
        };
        ProjectedTilePoint {
            point: Point::new(pos.x / pos.w, pos.y / pos.w),
            signed_distance_from_camera: pos.w,
            occluded,
        }
    }

    /// [`Projection::drawn_tile_point`](crate::Projection::drawn_tile_point) on the globe.
    pub(crate) fn drawn_tile_point(
        data: &ProjectionData,
        tile_id: &UnwrappedTileID,
        point: &Point<f64>,
        elevation: f64,
    ) -> ProjectedTilePoint {
        let globe = Self::project_tile_point(data, tile_id, point, elevation);
        let t = data.projection_transition;
        if t > 0.999 {
            return globe;
        }
        // The shaders' `interpolateProjection`: the sphere blended with
        // Mercator, the horizon clipping through the blended depth from a
        // fifth of the hand-over on. Raised geometry keeps its depth and the
        // globe hides it.
        let flat = data.fallback_matrix * DVec4::new(point.x, point.y, elevation, 1.0);
        let globe_w = globe.signed_distance_from_camera;
        let w = interpolate(&flat.w, &globe_w, t);
        let globeness = ((t - 0.2) / 0.8).clamp(0.0, 1.0);
        let mut occluded = globeness > 0.0 && globe.occluded;
        if elevation <= 0.0 {
            let surface = Self::tile_coordinates_to_sphere(point, tile_id);
            let clipping_z = 1.0 - point_plane_signed_distance(data.clipping_plane, surface);
            occluded = globeness * clipping_z * globe_w > w;
        }
        ProjectedTilePoint {
            point: Point::new(
                interpolate(&flat.x, &(globe.point.x * globe_w), t) / w,
                interpolate(&flat.y, &(globe.point.y * globe_w), t) / w,
            ),
            signed_distance_from_camera: w,
            occluded,
        }
    }

    /// [`Projection::circle_radius_correction`](crate::Projection::circle_radius_correction) on the globe.
    pub(crate) fn circle_radius_correction(view: &View) -> f64 {
        interpolate(
            &1.0,
            &Self::planet_scale(view.center(WrapMode::Unwrapped).latitude()),
            view.projection_transition(),
        )
    }

    /// [`Projection::pixel_scale`](crate::Projection::pixel_scale) on the globe.
    pub(crate) fn pixel_scale(view: &View) -> f64 {
        interpolate(
            &1.0,
            &(1.0 / Self::planet_scale(view.center(WrapMode::Unwrapped).latitude())),
            view.projection_transition(),
        )
    }

    /// [`Projection::pitched_text_correction`](crate::Projection::pitched_text_correction) on the globe.
    pub(crate) fn pitched_text_correction(view: &View, tile_anchor: &Point<f64>, tile_id: &UnwrappedTileID) -> f64 {
        let scale = 1.0 / (1u64 << tile_id.canonical.z) as f64;
        let mercator_y = tile_anchor.y / f64::from(EXTENT) * scale + f64::from(tile_id.canonical.y) * scale;
        let latitude = 2.0 * (PI - (mercator_y * PI * 2.0)).exp().atan() - PI * 0.5;
        interpolate(
            &1.0,
            &(Self::planet_scale(view.center(WrapMode::Unwrapped).latitude()) / latitude.cos()),
            view.projection_transition(),
        )
    }
}
