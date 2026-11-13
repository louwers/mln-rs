use geometry::Point;

use crate::mat3::{vec3_add, vec3_cross, vec3_dot, vec3_length, vec3_normalize, vec3_scale, vec3_sub};
use crate::mat4::{Mat4, matrix};
use crate::vectors::{Vec3, Vec3i, Vec4};

fn to_vec3(v: &Vec4) -> Vec3 {
    [v[0], v[1], v[2]]
}

fn vec4_dot(a: &Vec4, b: &Vec4) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

fn ray_plane_intersection(origin: &Vec3, direction: &Vec3, plane: &Vec4) -> Option<f64> {
    let normal: Vec3 = [plane[0], plane[1], plane[2]];
    let along = vec3_dot(direction, &normal);
    if along == 0.0 {
        return None;
    }
    Some((-vec3_dot(origin, &normal) - plane[3]) / along)
}

fn plane_through(p0: &Vec3, p1: &Vec3, p2: &Vec3) -> Vec4 {
    let n = vec3_normalize(&vec3_cross(&vec3_sub(p0, p1), &vec3_sub(p2, p1)));
    [n[0], n[1], n[2], -vec3_dot(&n, p1)]
}

/// The distance from the near plane to the point of the horizon circle
/// farthest along the view direction, if the camera is not looking straight
/// down.
fn ideal_near_far_plane_distance(horizon_plane: &Vec4, near_plane: &Vec4) -> Option<f64> {
    let view: Vec3 = [near_plane[0], near_plane[1], near_plane[2]];
    let horizon_length = vec3_length(&[horizon_plane[0], horizon_plane[1], horizon_plane[2]]);
    let horizon_normal = vec3_scale(&[horizon_plane[0], horizon_plane[1], horizon_plane[2]], 1.0 / horizon_length);
    let horizon_distance = horizon_plane[3] / horizon_length;
    let projected_view = vec3_sub(&view, &vec3_scale(&horizon_normal, vec3_dot(&view, &horizon_normal)));
    let projected_length = vec3_length(&projected_view);
    if projected_length <= 0.0 {
        return None;
    }
    let circle_radius = (1.0 - horizon_distance * horizon_distance).sqrt();
    let circle_center = vec3_scale(&horizon_normal, -horizon_distance);
    let farthest = vec3_add(&circle_center, &vec3_scale(&projected_view, circle_radius / projected_length));
    Some(vec3_dot(&view, &farthest) + near_plane[3])
}

/// Moves the far corners in along the frustum's edges until the far plane
/// reaches no further than the horizon; port of GL JS
/// `adjustFarPlaneByHorizonPlane`. The near corners are the first four.
fn adjust_far_plane_by_horizon_plane(corners: &mut [Vec4; 8], near_plane_indices: &Vec3i, horizon_plane: &Vec4) {
    const NEAR_OFFSET: usize = 0;
    const FAR_OFFSET: usize = 4;
    let mut ray_lengths = [0.0f64; 4];
    let mut ray_directions: [Vec3; 4] = [[0.0; 3]; 4];
    let mut max_distance = 0.0;
    for i in 0..4 {
        let ray = vec3_sub(&to_vec3(&corners[i + FAR_OFFSET]), &to_vec3(&corners[i + NEAR_OFFSET]));
        ray_lengths[i] = vec3_length(&ray);
        ray_directions[i] = vec3_scale(&ray, 1.0 / ray_lengths[i]);
    }
    for i in 0..4 {
        let distance = ray_plane_intersection(&to_vec3(&corners[i + NEAR_OFFSET]), &ray_directions[i], horizon_plane);
        // Rays parallel to the horizon or pointing away from it keep their
        // length.
        let length = match distance {
            Some(distance) if distance >= 0.0 => distance,
            _ => ray_lengths[i],
        };
        max_distance = f64::max(max_distance, length);
    }
    let near_plane = plane_through(
        &to_vec3(&corners[near_plane_indices[0] as usize]),
        &to_vec3(&corners[near_plane_indices[1] as usize]),
        &to_vec3(&corners[near_plane_indices[2] as usize]),
    );
    if let Some(ideal) = ideal_near_far_plane_distance(horizon_plane, &near_plane) {
        // The rays make the same angle with the near plane at all four
        // corners.
        max_distance = f64::min(
            max_distance,
            ideal / vec3_dot(&ray_directions[0], &[near_plane[0], near_plane[1], near_plane[2]]),
        );
    }
    for i in 0..4 {
        let far = vec3_add(
            &to_vec3(&corners[i + NEAR_OFFSET]),
            &vec3_scale(&ray_directions[i], f64::min(max_distance, ray_lengths[i])),
        );
        corners[i + FAR_OFFSET] = [far[0], far[1], far[2], 1.0];
    }
}

fn project_points_to_axis<const N: usize>(points: &[Vec3; N], origin: &Vec3, axis: &Vec3) -> Point<f64> {
    let mut min = f64::MAX;
    let mut max = -f64::MAX;

    for point in points {
        let projected_point = vec3_dot(&vec3_sub(point, origin), axis);
        min = f64::min(projected_point, min);
        max = f64::max(projected_point, max);
    }

    Point::new(min, max)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum IntersectionResult {
    Separate,
    Intersects,
    Contains,
}

/// The derived `PartialEq` is the C++ operator==: `min == aabb.min && max ==
/// aabb.max` (`std::array` element-wise ==).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AABB {
    pub min: Vec3,
    pub max: Vec3,
}

impl AABB {
    #[must_use]
    pub fn new(min: Vec3, max: Vec3) -> Self {
        AABB { min, max }
    }

    #[must_use]
    pub fn closest_point(&self, point: &Vec3) -> Vec3 {
        [
            f64::max(f64::min(self.max[0], point[0]), self.min[0]),
            f64::max(f64::min(self.max[1], point[1]), self.min[1]),
            f64::max(f64::min(self.max[2], point[2]), self.min[2]),
        ]
    }

    /// Computes the shortest manhattan distance to the provided point
    #[must_use]
    pub fn distance_xyz(&self, point: &Vec3) -> Vec3 {
        let mut vec = vec3_sub(&self.closest_point(point), point);

        vec[0] = vec[0].abs();
        vec[1] = vec[1].abs();
        vec[2] = vec[2].abs();

        vec
    }

    /// Creates an aabb covering one quadrant of the aabb on XZ-plane.
    #[must_use]
    pub fn quadrant(&self, idx: i32) -> AABB {
        debug_assert!((0..4).contains(&idx));
        let mut quadrant_min = self.min;
        let mut quadrant_max = self.max;
        let x_center = f64::midpoint(self.max[0], self.min[0]);
        let y_center = f64::midpoint(self.max[1], self.min[1]);

        // This aabb is split into 4 quadrants. For each axis define in which side
        // of the split "idx" is The result for indices 0, 1, 2, 3 is: { 0, 0 }, {
        // 1, 0 }, { 0, 1 }, { 1, 1 }
        let x_split: [i32; 4] = [0, 1, 0, 1];
        let y_split: [i32; 4] = [0, 0, 1, 1];
        let idx = idx as usize;

        quadrant_min[0] = if x_split[idx] != 0 { x_center } else { quadrant_min[0] };
        quadrant_max[0] = if x_split[idx] != 0 { quadrant_max[0] } else { x_center };

        quadrant_min[1] = if y_split[idx] != 0 { y_center } else { quadrant_min[1] };
        quadrant_max[1] = if y_split[idx] != 0 { quadrant_max[1] } else { y_center };

        AABB::new(quadrant_min, quadrant_max)
    }

    #[must_use]
    pub fn intersects(&self, aabb: &AABB) -> bool {
        if self.min[0] > aabb.max[0] || aabb.min[0] > self.max[0] {
            return false;
        }
        if self.min[1] > aabb.max[1] || aabb.min[1] > self.max[1] {
            return false;
        }
        if self.min[2] > aabb.max[2] || aabb.min[2] > self.max[2] {
            return false;
        }
        true
    }
}

// Named index values for frustum::points array
const NEAR_TL: i32 = 0;
const NEAR_TR: i32 = 1;
const NEAR_BR: i32 = 2;
const NEAR_BL: i32 = 3;
const FAR_TL: i32 = 4;
const FAR_TR: i32 = 5;
const FAR_BR: i32 = 6;
const FAR_BL: i32 = 7;

#[derive(Clone, Copy, Debug, Default)]
struct Projection {
    axis: Vec3,
    projection: Point<f64>,
}

#[derive(Clone, Debug)]
pub struct Frustum {
    bounds: AABB,
    points: [Vec3; 8],
    planes: [Vec4; 6],
    projections: [Projection; 12],
}

impl Frustum {
    #[must_use]
    pub fn new(points: &[Vec3; 8], planes: &[Vec4; 6]) -> Self {
        let points = *points;
        let planes = *planes;

        let x_bounds = project_points_to_axis(&points, &[0.0, 0.0, 0.0], &[1.0, 0.0, 0.0]);
        let y_bounds = project_points_to_axis(&points, &[0.0, 0.0, 0.0], &[0.0, 1.0, 0.0]);
        let z_bounds = project_points_to_axis(&points, &[0.0, 0.0, 0.0], &[0.0, 0.0, 1.0]);

        let bounds = AABB::new([x_bounds.x, y_bounds.x, z_bounds.x], [x_bounds.y, y_bounds.y, z_bounds.y]);

        // Precompute a set of separating axis candidates for precise intersection
        // tests. Remaining axes not covered in basic intersection tests are: axis[]
        // = (edges of aabb) x (edges of frustum)
        let p = |i: i32| &points[i as usize];
        let frustum_edges: [Vec3; 6] = [
            vec3_sub(p(NEAR_BR), p(NEAR_BL)),
            vec3_sub(p(NEAR_TL), p(NEAR_BL)),
            vec3_sub(p(FAR_TL), p(NEAR_TL)),
            vec3_sub(p(FAR_TR), p(NEAR_TR)),
            vec3_sub(p(FAR_BR), p(NEAR_BR)),
            vec3_sub(p(FAR_BL), p(NEAR_BL)),
        ];

        let mut projections = [Projection::default(); 12];
        for (i, edge) in frustum_edges.iter().enumerate() {
            // Cross product [1, 0, 0] x [a, b, c] == [0, -c, b]
            // Cross product [0, 1, 0] x [a, b, c] == [c, 0, -a]
            let axis0: Vec3 = [0.0, -edge[2], edge[1]];
            let axis1: Vec3 = [edge[2], 0.0, -edge[0]];

            projections[i * 2] =
                Projection { axis: axis0, projection: project_points_to_axis(&points, &points[0], &axis0) };
            projections[i * 2 + 1] =
                Projection { axis: axis1, projection: project_points_to_axis(&points, &points[0], &axis1) };
        }

        Frustum { bounds, points, planes, projections }
    }

    /// With `horizon_plane`, the far plane is pulled in to the horizon so
    /// nothing behind it counts as visible.
    #[must_use]
    pub fn from_inv_proj_matrix(
        inv_proj: &Mat4,
        world_size: f64,
        zoom: f64,
        flipped_y: bool,
        horizon_plane: Option<&Vec4>,
    ) -> Frustum {
        // Define frustum corner points in normalized clip space
        let mut corner_coords: [Vec4; 8] = [
            [-1.0, 1.0, -1.0, 1.0],
            [1.0, 1.0, -1.0, 1.0],
            [1.0, -1.0, -1.0, 1.0],
            [-1.0, -1.0, -1.0, 1.0],
            [-1.0, 1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
            [1.0, -1.0, 1.0, 1.0],
            [-1.0, -1.0, 1.0, 1.0],
        ];

        let scale = 2.0f64.powf(zoom);

        // Transform points to tile space
        for coord in &mut corner_coords {
            let a = *coord;
            matrix::transform_mat4(coord, &a, inv_proj);
            // `component *= 1.0 / coord[3] / worldSize * scale` for each
            // component in order: coord[3] is the last one written.
            for i in 0..4 {
                coord[i] *= 1.0 / coord[3] / world_size * scale;
            }
        }

        let mut frustum_plane_point_indices: [Vec3i; 6] = [
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
            adjust_far_plane_by_horizon_plane(&mut corner_coords, &frustum_plane_point_indices[4], horizon_plane);
        }

        let mut frustum_planes: [Vec4; 6] = [[0.0; 4]; 6];

        for (i, indices) in frustum_plane_point_indices.iter().enumerate() {
            frustum_planes[i] = plane_through(
                &to_vec3(&corner_coords[indices[0] as usize]),
                &to_vec3(&corner_coords[indices[1] as usize]),
                &to_vec3(&corner_coords[indices[2] as usize]),
            );
        }

        let mut frustum_points: [Vec3; 8] = [[0.0; 3]; 8];

        for i in 0..corner_coords.len() {
            frustum_points[i] = to_vec3(&corner_coords[i]);
        }

        Frustum::new(&frustum_points, &frustum_planes)
    }

    /// Performs conservative intersection test using separating axis theorem.
    /// Some accuracy is traded for better performance. False positive rate is < 1%
    #[must_use]
    pub fn intersects(&self, aabb: &AABB) -> IntersectionResult {
        // Execute separating axis test between two convex objects to find intersections
        // Each frustum plane together with 3 major axes define the separating axes
        // This implementation is conservative as it's not checking all possible axes.
        // False positive rate is ~0.5% of all cases (see intersectsPrecise).
        // Note: test only 4 points as both min and max points have zero elevation
        debug_assert!(aabb.min[2] == 0.0 && aabb.max[2] == 0.0);

        if !self.bounds.intersects(aabb) {
            return IntersectionResult::Separate;
        }

        let aabb_points: [Vec4; 4] = [
            [aabb.min[0], aabb.min[1], 0.0, 1.0],
            [aabb.max[0], aabb.min[1], 0.0, 1.0],
            [aabb.max[0], aabb.max[1], 0.0, 1.0],
            [aabb.min[0], aabb.max[1], 0.0, 1.0],
        ];

        let mut fully_inside = true;

        let epsilon = 1e-10;

        for plane in &self.planes {
            let mut points_inside: usize = 0;

            points_inside += usize::from(vec4_dot(plane, &aabb_points[0]) >= -epsilon);
            points_inside += usize::from(vec4_dot(plane, &aabb_points[1]) >= -epsilon);
            points_inside += usize::from(vec4_dot(plane, &aabb_points[2]) >= -epsilon);
            points_inside += usize::from(vec4_dot(plane, &aabb_points[3]) >= -epsilon);

            if points_inside == 0 {
                // Separating axis found, no intersection
                return IntersectionResult::Separate;
            }

            if points_inside != aabb_points.len() {
                fully_inside = false;
            }
        }

        if fully_inside { IntersectionResult::Contains } else { IntersectionResult::Intersects }
    }

    /// Performs precise intersection test using separating axis theorem.
    /// It is possible run only edge cases that were not covered in `intersects()`
    /// (C++ default: `edgeCasesOnly = false`).
    #[must_use]
    pub fn intersects_precise(&self, aabb: &AABB, edge_cases_only: bool) -> IntersectionResult {
        if !edge_cases_only {
            let result = self.intersects(aabb);

            if result == IntersectionResult::Separate {
                return result;
            }
        }

        let aabb_points: [Vec3; 4] = [
            [aabb.min[0], aabb.min[1], 0.0],
            [aabb.max[0], aabb.min[1], 0.0],
            [aabb.max[0], aabb.max[1], 0.0],
            [aabb.min[0], aabb.max[1], 0.0],
        ];

        // For a precise SAT-test all edge cases needs to be covered
        // Projections of the frustum on separating axis candidates have been precomputed already
        for proj in &self.projections {
            let projected_aabb = project_points_to_axis(&aabb_points, &self.points[0], &proj.axis);
            let projected_frustum = &proj.projection;

            if projected_frustum.y < projected_aabb.x || projected_frustum.x > projected_aabb.y {
                return IntersectionResult::Separate;
            }
        }

        IntersectionResult::Intersects
    }

    #[must_use]
    pub fn get_points(&self) -> &[Vec3; 8] {
        &self.points
    }

    #[must_use]
    pub fn get_planes(&self) -> &[Vec4; 6] {
        &self.planes
    }
}

#[cfg(test)]
#[path = "bounding_volumes_test.rs"]
mod bounding_volumes_test;
