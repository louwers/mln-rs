use geometry::Point;
use glam::{DMat4, DVec3, DVec4, Vec4Swizzles};

fn ray_plane_intersection(origin: DVec3, direction: DVec3, plane: DVec4) -> Option<f64> {
    let normal = plane.xyz();
    let along = direction.dot(normal);
    if along == 0.0 {
        return None;
    }
    Some((-origin.dot(normal) - plane.w) / along)
}

/// Where a point is from a plane, as `ax + by + cz + d`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlaneSide {
    Inside,
    /// On the plane, within `rounding`.
    On,
    Outside,
}

fn plane_side(plane: DVec4, point: DVec4, rounding: f64) -> PlaneSide {
    let distance = plane.dot(point);
    if distance > rounding {
        PlaneSide::Inside
    } else if distance < -rounding {
        PlaneSide::Outside
    } else {
        PlaneSide::On
    }
}

fn plane_through(p0: DVec3, p1: DVec3, p2: DVec3) -> DVec4 {
    let n = (p0 - p1).cross(p2 - p1).normalize();
    n.extend(-n.dot(p1))
}

/// The distance from the near plane to the point of the horizon circle
/// farthest along the view direction, if the camera is not looking straight
/// down.
fn ideal_near_far_plane_distance(horizon_plane: DVec4, near_plane: DVec4) -> Option<f64> {
    let view = near_plane.xyz();
    let horizon_length = horizon_plane.xyz().length();
    let horizon_normal = horizon_plane.xyz() / horizon_length;
    let horizon_distance = horizon_plane.w / horizon_length;
    let projected_view = view - horizon_normal * view.dot(horizon_normal);
    let projected_length = projected_view.length();
    if projected_length <= 0.0 {
        return None;
    }
    let circle_radius = (1.0 - horizon_distance * horizon_distance).sqrt();
    let circle_center = horizon_normal * -horizon_distance;
    let farthest = circle_center + projected_view * (circle_radius / projected_length);
    Some(view.dot(farthest) + near_plane.w)
}

/// Moves the far corners in along the frustum's edges until the far plane
/// reaches no further than the horizon, as GL JS's
/// `adjustFarPlaneByHorizonPlane` does. The near corners are the first four.
fn adjust_far_plane_by_horizon_plane(corners: &mut [DVec3; 8], near_plane_indices: [usize; 3], horizon_plane: DVec4) {
    const NEAR_OFFSET: usize = 0;
    const FAR_OFFSET: usize = 4;
    let mut ray_lengths = [0.0f64; 4];
    let mut ray_directions = [DVec3::ZERO; 4];
    let mut max_distance = 0.0;
    for i in 0..4 {
        let ray = corners[i + FAR_OFFSET] - corners[i + NEAR_OFFSET];
        ray_lengths[i] = ray.length();
        ray_directions[i] = ray / ray_lengths[i];
    }
    for i in 0..4 {
        let distance = ray_plane_intersection(corners[i + NEAR_OFFSET], ray_directions[i], horizon_plane);
        // Rays parallel to the horizon or pointing away from it keep their
        // length.
        let length = match distance {
            Some(distance) if distance >= 0.0 => distance,
            _ => ray_lengths[i],
        };
        max_distance = f64::max(max_distance, length);
    }
    let [a, b, c] = near_plane_indices;
    let near_plane = plane_through(corners[a], corners[b], corners[c]);
    if let Some(ideal) = ideal_near_far_plane_distance(horizon_plane, near_plane) {
        // The rays make the same angle with the near plane at all four
        // corners.
        max_distance = f64::min(max_distance, ideal / ray_directions[0].dot(near_plane.xyz()));
    }
    for i in 0..4 {
        corners[i + FAR_OFFSET] = corners[i + NEAR_OFFSET] + ray_directions[i] * f64::min(max_distance, ray_lengths[i]);
    }
}

fn project_points_to_axis(points: &[DVec3], origin: DVec3, axis: DVec3) -> Point<f64> {
    let mut min = f64::MAX;
    let mut max = -f64::MAX;

    for point in points {
        let projected_point = (*point - origin).dot(axis);
        min = f64::min(projected_point, min);
        max = f64::max(projected_point, max);
    }

    Point::new(min, max)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntersectionResult {
    Separate,
    Intersects,
    Contains,
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Aabb {
    pub(crate) min: DVec3,
    pub(crate) max: DVec3,
}

impl Aabb {
    #[must_use]
    pub(crate) fn new(min: DVec3, max: DVec3) -> Self {
        Aabb { min, max }
    }

    #[must_use]
    pub(crate) fn closest_point(&self, point: DVec3) -> DVec3 {
        point.min(self.max).max(self.min)
    }

    /// The distance along each axis from `point` to the box, 0 on the axes
    /// where the point is within it.
    #[must_use]
    pub(crate) fn distance_xyz(&self, point: DVec3) -> DVec3 {
        (self.closest_point(point) - point).abs()
    }

    /// The quadrant `idx` of the box on its XY-plane: 0 is the minimum x and
    /// y, 1 the maximum x, 2 the maximum y and 3 both maxima.
    #[must_use]
    pub(crate) fn quadrant(&self, idx: usize) -> Aabb {
        debug_assert!(idx < 4);
        let center = (self.min + self.max) * 0.5;
        let mut min = self.min;
        let mut max = self.max;
        if idx & 1 == 0 {
            max.x = center.x;
        } else {
            min.x = center.x;
        }
        if idx & 2 == 0 {
            max.y = center.y;
        } else {
            min.y = center.y;
        }
        Aabb::new(min, max)
    }

    #[must_use]
    pub(crate) fn intersects(&self, aabb: &Aabb) -> bool {
        self.min.cmple(aabb.max).all() && aabb.min.cmple(self.max).all()
    }
}

// Named indices of `Frustum::points`.
const NEAR_TL: usize = 0;
const NEAR_TR: usize = 1;
const NEAR_BR: usize = 2;
const NEAR_BL: usize = 3;
const FAR_TL: usize = 4;
const FAR_TR: usize = 5;
const FAR_BR: usize = 6;
const FAR_BL: usize = 7;

#[derive(Clone, Copy, Debug, Default)]
struct Projection {
    axis: DVec3,
    projection: Point<f64>,
}

#[derive(Clone, Debug)]
pub(crate) struct Frustum {
    bounds: Aabb,
    points: [DVec3; 8],
    planes: [DVec4; 6],
    projections: [Projection; 12],
}

impl Frustum {
    #[must_use]
    pub(crate) fn new(points: [DVec3; 8], planes: [DVec4; 6]) -> Self {
        let x_bounds = project_points_to_axis(&points, DVec3::ZERO, DVec3::X);
        let y_bounds = project_points_to_axis(&points, DVec3::ZERO, DVec3::Y);
        let z_bounds = project_points_to_axis(&points, DVec3::ZERO, DVec3::Z);

        let bounds =
            Aabb::new(DVec3::new(x_bounds.x, y_bounds.x, z_bounds.x), DVec3::new(x_bounds.y, y_bounds.y, z_bounds.y));

        // Precompute a set of separating axis candidates for precise intersection
        // tests. Remaining axes not covered in basic intersection tests are: axis[]
        // = (edges of aabb) x (edges of frustum)
        let p = points;
        let frustum_edges = [
            p[NEAR_BR] - p[NEAR_BL],
            p[NEAR_TL] - p[NEAR_BL],
            p[FAR_TL] - p[NEAR_TL],
            p[FAR_TR] - p[NEAR_TR],
            p[FAR_BR] - p[NEAR_BR],
            p[FAR_BL] - p[NEAR_BL],
        ];

        let mut projections = [Projection::default(); 12];
        for (i, edge) in frustum_edges.iter().enumerate() {
            for (j, axis) in [DVec3::X.cross(*edge), DVec3::Y.cross(*edge)].into_iter().enumerate() {
                projections[i * 2 + j] =
                    Projection { axis, projection: project_points_to_axis(&points, points[0], axis) };
            }
        }

        Frustum { bounds, points, planes, projections }
    }

    /// The frustum that `inv_proj` takes clip space's cube to, in the tile
    /// units of `zoom`. With `horizon_plane`, the far plane is pulled in to
    /// the horizon so nothing behind it counts as visible.
    #[must_use]
    pub(crate) fn from_inv_proj_matrix(
        inv_proj: &DMat4,
        world_size: f64,
        zoom: f64,
        flipped_y: bool,
        horizon_plane: Option<DVec4>,
    ) -> Frustum {
        let corners = Self::clip_corners(inv_proj).map(|corner| corner / world_size * 2.0f64.powf(zoom));
        Self::from_corners(corners, flipped_y, horizon_plane)
    }

    /// The frustum that `inv_proj` takes clip space's cube to, with x and y
    /// in the tile units of `zoom` and z left in the units `inv_proj` gives
    /// it, meters for a map's projection matrix (GL JS `createFrustum`). Boxes
    /// tested against it with [`intersects_box`](Self::intersects_box) take
    /// the same units.
    #[must_use]
    pub(crate) fn from_inv_proj_matrix_metres(
        inv_proj: &DMat4,
        world_size: f64,
        zoom: f64,
        flipped_y: bool,
    ) -> Frustum {
        let scale = 2.0f64.powf(zoom);
        let corners = Self::clip_corners(inv_proj).map(|corner| {
            let xy = corner / world_size * scale;
            DVec3::new(xy.x, xy.y, corner.z)
        });
        Self::from_corners(corners, flipped_y, None)
    }

    /// The corners of clip space's cube through `inv_proj`.
    fn clip_corners(inv_proj: &DMat4) -> [DVec3; 8] {
        // The frustum's corners in normalized clip space.
        let clip_corners = [
            DVec3::new(-1.0, 1.0, -1.0),
            DVec3::new(1.0, 1.0, -1.0),
            DVec3::new(1.0, -1.0, -1.0),
            DVec3::new(-1.0, -1.0, -1.0),
            DVec3::new(-1.0, 1.0, 1.0),
            DVec3::new(1.0, 1.0, 1.0),
            DVec3::new(1.0, -1.0, 1.0),
            DVec3::new(-1.0, -1.0, 1.0),
        ];
        clip_corners.map(|corner| inv_proj.project_point3(corner))
    }

    fn from_corners(mut corners: [DVec3; 8], flipped_y: bool, horizon_plane: Option<DVec4>) -> Frustum {
        let mut frustum_plane_point_indices = [
            [NEAR_BL, NEAR_BR, FAR_BR],  // bottom
            [NEAR_TL, NEAR_BL, FAR_BL],  // left
            [NEAR_BR, NEAR_TR, FAR_TR],  // right
            [NEAR_TL, FAR_TL, FAR_TR],   // top
            [NEAR_TL, NEAR_TR, NEAR_BR], // near
            [FAR_BR, FAR_TR, FAR_TL],    // far
        ];

        if flipped_y {
            for tri in &mut frustum_plane_point_indices {
                tri.swap(1, 2);
            }
        }

        if let Some(horizon_plane) = horizon_plane {
            adjust_far_plane_by_horizon_plane(&mut corners, frustum_plane_point_indices[4], horizon_plane);
        }

        let planes = frustum_plane_point_indices.map(|[a, b, c]| plane_through(corners[a], corners[b], corners[c]));
        Frustum::new(corners, planes)
    }

    /// A conservative intersection test by the separating axis theorem: it
    /// may report an intersection where there is none (in under 1% of cases)
    /// for speed. A box that only touches the frustum is separate from it,
    /// and one that touches it from inside is within it. `aabb` must lie on
    /// the plane z = 0.
    #[must_use]
    pub(crate) fn intersects(&self, aabb: &Aabb) -> IntersectionResult {
        // Each frustum plane together with 3 major axes define the separating
        // axes. Both points of the box have zero elevation, so testing its 4
        // corners on the plane is enough.
        debug_assert!(aabb.min.z == 0.0 && aabb.max.z == 0.0);

        if !self.bounds.intersects(aabb) {
            return IntersectionResult::Separate;
        }

        let aabb_points = [
            DVec4::new(aabb.min.x, aabb.min.y, 0.0, 1.0),
            DVec4::new(aabb.max.x, aabb.min.y, 0.0, 1.0),
            DVec4::new(aabb.max.x, aabb.max.y, 0.0, 1.0),
            DVec4::new(aabb.min.x, aabb.max.y, 0.0, 1.0),
        ];

        let mut fully_inside = true;

        // A plane is placed as precisely as the coordinates of the points it
        // passes through, which along each axis are as large as the frustum's.
        let magnitude = self.points.iter().fold(DVec3::ZERO, |magnitude, point| magnitude.max(point.abs()));
        for plane in &self.planes {
            let rounding = 1e-12 * plane.xyz().abs().dot(magnitude);
            let sides = aabb_points.map(|point| plane_side(*plane, point, rounding));
            if !sides.contains(&PlaneSide::Inside) {
                // Separating axis found, no intersection
                return IntersectionResult::Separate;
            }
            if sides.contains(&PlaneSide::Outside) {
                fully_inside = false;
            }
        }

        if fully_inside { IntersectionResult::Contains } else { IntersectionResult::Intersects }
    }

    /// A precise intersection test by the separating axis theorem. With
    /// `edge_cases_only`, it tests only the axes that
    /// [`intersects`](Self::intersects) doesn't.
    #[must_use]
    pub(crate) fn intersects_precise(&self, aabb: &Aabb, edge_cases_only: bool) -> IntersectionResult {
        if !edge_cases_only {
            let result = self.intersects(aabb);

            if result == IntersectionResult::Separate {
                return result;
            }
        }

        let aabb_points = [
            DVec3::new(aabb.min.x, aabb.min.y, 0.0),
            DVec3::new(aabb.max.x, aabb.min.y, 0.0),
            DVec3::new(aabb.max.x, aabb.max.y, 0.0),
            DVec3::new(aabb.min.x, aabb.max.y, 0.0),
        ];

        // For a precise SAT-test all edge cases needs to be covered
        // Projections of the frustum on separating axis candidates have been precomputed already
        for proj in &self.projections {
            let projected_aabb = project_points_to_axis(&aabb_points, self.points[0], proj.axis);
            let projected_frustum = &proj.projection;

            if projected_frustum.y < projected_aabb.x || projected_frustum.x > projected_aabb.y {
                return IntersectionResult::Separate;
            }
        }

        IntersectionResult::Intersects
    }

    /// A conservative intersection test of a box of any height, as GL JS's
    /// `Aabb.intersectsFrustum`: the box is separate if all of it is behind
    /// one plane, or outside the frustum's bounds along an axis. It may
    /// report an intersection where there is none. A box that only touches a
    /// plane from outside intersects it.
    #[must_use]
    pub(crate) fn intersects_box(&self, aabb: &Aabb) -> IntersectionResult {
        let mut fully_inside = true;
        for plane in &self.planes {
            // The plane's distance to the box's corners nearest and farthest
            // along its normal, summed in GL JS's order.
            let mut distance_min = plane.w;
            let mut distance_max = plane.w;
            for axis in 0..3 {
                let (near, far) =
                    if plane[axis] > 0.0 { (aabb.min[axis], aabb.max[axis]) } else { (aabb.max[axis], aabb.min[axis]) };
                distance_min += plane[axis] * near;
                distance_max += plane[axis] * far;
            }
            if distance_max < 0.0 {
                return IntersectionResult::Separate;
            }
            if distance_min < 0.0 {
                fully_inside = false;
            }
        }
        if fully_inside {
            return IntersectionResult::Contains;
        }
        if !self.bounds.intersects(aabb) {
            return IntersectionResult::Separate;
        }
        IntersectionResult::Intersects
    }

    #[must_use]
    pub(crate) fn points(&self) -> &[DVec3; 8] {
        &self.points
    }

    #[must_use]
    pub(crate) fn planes(&self) -> &[DVec4; 6] {
        &self.planes
    }

    /// This frustum with its far plane `distance` farther from `camera`, GL
    /// JS's `pushFrustumFarPlane`. Each far corner moves along its ray from
    /// the camera to the moved plane, so the frustum stays a pyramid.
    #[must_use]
    pub(crate) fn with_far_plane_pushed(&self, distance: f64, camera: DVec3) -> Frustum {
        const FAR: usize = 5;
        let normal = self.planes[FAR].xyz();
        let mut points = self.points;
        for point in &mut points[FAR_TL..=FAR_BL] {
            let ray = *point - camera;
            // The plane's normal points back into the frustum.
            let ray_dot_normal = ray.dot(normal);
            if ray_dot_normal < -1e-9 {
                *point += ray * (distance / -ray_dot_normal);
            }
        }
        let mut planes = self.planes;
        planes[FAR].w += distance;
        Frustum::new(points, planes)
    }
}

#[cfg(test)]
#[path = "bounding_volumes_test.rs"]
mod bounding_volumes_test;
