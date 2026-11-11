// Ports test/util/bounding_volumes.test.cpp

use std::f64::consts::PI;

use super::*;

// gtest: BoundingVolumes.CreateAabb
#[test]
fn create_aabb() {
    let aabb = AABB::new([0.0, 0.0, 0.0], [2.0, 4.0, 6.0]);

    assert_eq!(aabb.min, [0.0, 0.0, 0.0]);
    assert_eq!(aabb.max, [2.0, 4.0, 6.0]);
}

// gtest: BoundingVolumes.AabbEquality
#[test]
fn aabb_equality() {
    let aabb0 = AABB::new([-4.23, 1.01, 2.0], [-1.0, 1.02, 100.0]);
    let aabb1 = AABB::new([-4.23, 1.01, 2.0], [-1.0, 1.02, 100.0]);
    let aabb2 = AABB::new([-4.23, 1.01, 2.0], [-1.0, 1.03, 55.0]);

    assert_eq!(aabb0, aabb1);
    assert_ne!(aabb0, aabb2);
}

// gtest: BoundingVolumes.CreateAabbQuadrants
#[test]
fn create_aabb_quadrants() {
    let aabb = AABB::new([0.0, 0.0, 0.0], [2.0, 4.0, 16.0]);

    let quadrant0 = AABB::new([0.0, 0.0, 0.0], [1.0, 2.0, 16.0]);
    let quadrant1 = AABB::new([1.0, 0.0, 0.0], [2.0, 2.0, 16.0]);
    let quadrant2 = AABB::new([0.0, 2.0, 0.0], [1.0, 4.0, 16.0]);
    let quadrant3 = AABB::new([1.0, 2.0, 0.0], [2.0, 4.0, 16.0]);

    assert_eq!(aabb.quadrant(0), quadrant0);
    assert_eq!(aabb.quadrant(1), quadrant1);
    assert_eq!(aabb.quadrant(2), quadrant2);
    assert_eq!(aabb.quadrant(3), quadrant3);
}

// gtest: BoundingVolumes.AabbDistanceToPoint
#[test]
fn aabb_distance_to_point() {
    let aabb = AABB::new([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]);

    assert_eq!(aabb.distance_xyz(&[0.5, -0.5, 0.5]), [0.0, 0.0, 0.0]);
    assert_eq!(aabb.distance_xyz(&[1.0, 1.0, 1.0]), [0.0, 0.0, 0.0]);
    assert_eq!(aabb.distance_xyz(&[0.0, 10.0, 0.0]), [0.0, 9.0, 0.0]);
    assert_eq!(aabb.distance_xyz(&[-2.0, -2.0, -4.0]), [1.0, 1.0, 3.0]);
}

// gtest: BoundingVolumes.AabbAabbIntersection
#[test]
fn aabb_aabb_intersection() {
    let aabb0 = AABB::new([1.0, 0.0, 0.0], [3.0, 2.0, 1.0]);
    let aabb1 = AABB::new([2.5, 1.5, -1.0], [6.0, 2.5, 1.0]);
    let aabb2 = AABB::new([3.5, -2.0, -2.0], [5.0, 1.0, -1.5]);
    let aabb3 = AABB::new([0.5, -1.0, -1.0], [6.5, 3.0, 4.0]);

    assert!(aabb0.intersects(&aabb1));
    assert!(!aabb0.intersects(&aabb2));
    assert!(!aabb1.intersects(&aabb2));
    assert!(aabb3.intersects(&aabb0));
    assert!(aabb3.intersects(&aabb1));
    assert!(!aabb3.intersects(&aabb2));
}

// gtest: BoundingVolumes.CreateFrustumFromProjectionMatrix
#[test]
fn create_frustum_from_projection_matrix() {
    let mut proj_matrix: Mat4 = [0.0; 16];
    let mut inv_proj_matrix: Mat4 = [0.0; 16];
    matrix::perspective(&mut proj_matrix, PI / 2.0, 1.0, 0.1, 100.0);
    // C++ ignores the result.
    let _ = matrix::invert(&mut inv_proj_matrix, &proj_matrix);

    let frustum = Frustum::from_inv_proj_matrix(&inv_proj_matrix, 1.0, 0.0, false, None);

    let expected_points: [Vec3; 8] = [
        [-0.1, 0.1, -0.1],
        [0.1, 0.1, -0.1],
        [0.1, -0.1, -0.1],
        [-0.1, -0.1, -0.1],
        [-100.0, 100.0, -100.0],
        [100.0, 100.0, -100.0],
        [100.0, -100.0, -100.0],
        [-100.0, -100.0, -100.0],
    ];

    let expected_planes: [Vec4; 6] = [
        [0.0, -0.707, 0.707, 0.0],
        [-0.707, 0.0, 0.707, 0.0],
        [0.707, 0.0, 0.707, 0.0],
        [0.0, 0.707, 0.707, 0.0],
        [0.0, 0.0, 1.0, 0.1],
        [0.0, 0.0, -1.0, -100.0],
    ];

    // C `round`: half away from zero, like f64::round.
    let round_point = |v: &mut Vec3| {
        for p in v.iter_mut() {
            *p = (*p * 10.0).round() / 10.0;
        }
    };

    let round_plane = |v: &mut Vec4| {
        for p in v.iter_mut() {
            *p = (*p * 1000.0).round() / 1000.0;
        }
    };

    let mut actual_points = *frustum.get_points();
    let mut actual_planes = *frustum.get_planes();

    actual_points.iter_mut().for_each(round_point);
    actual_planes.iter_mut().for_each(round_plane);

    assert_eq!(actual_points, expected_points);
    assert_eq!(actual_planes, expected_planes);
}

fn create_test_frustum(fovy: f64, aspect_ratio: f64, z_near: f64, z_far: f64, elevation: f64, bearing: f64) -> Frustum {
    let mut proj: Mat4 = [0.0; 16];
    let mut inv_proj: Mat4 = [0.0; 16];

    // Create a complete projection matrix for transforming from world space to clip space
    matrix::perspective(&mut proj, fovy, aspect_ratio, z_near, z_far);
    let a = proj;
    matrix::scale(&mut proj, &a, 1.0, -1.0, 1.0);
    let a = proj;
    matrix::translate(&mut proj, &a, 0.0, 0.0, elevation);
    let a = proj;
    matrix::rotate_z(&mut proj, &a, bearing);
    // C++ ignores the result.
    let _ = matrix::invert(&mut inv_proj, &proj);

    Frustum::from_inv_proj_matrix(&inv_proj, 1.0, 0.0, false, None)
}

// gtest: BoundingVolumes.AabbFullyInsideFrustum
#[test]
fn aabb_fully_inside_frustum() {
    let frustum = create_test_frustum(PI / 2.0, 1.0, 0.1, 100.0, -5.0, 0.0);

    let aabb_array: [AABB; 3] = [
        AABB::new([-1.0, -1.0, 0.0], [1.0, 1.0, 0.0]),
        AABB::new([-5.0, -5.0, 0.0], [5.0, 5.0, 0.0]),
        AABB::new([-5.0, -5.0, 0.0], [-4.0, -2.0, 0.0]),
    ];

    assert_eq!(frustum.intersects(&aabb_array[0]), IntersectionResult::Contains);
    assert_eq!(frustum.intersects(&aabb_array[1]), IntersectionResult::Contains);
    assert_eq!(frustum.intersects(&aabb_array[2]), IntersectionResult::Contains);
}

// gtest: BoundingVolumes.AabbIntersectsFrustum
#[test]
fn aabb_intersects_frustum() {
    let frustum = create_test_frustum(PI / 2.0, 1.0, 0.1, 100.0, -5.0, 0.0);

    let aabb_array: [AABB; 2] =
        [AABB::new([-6.0, -6.0, 0.0], [6.0, 6.0, 0.0]), AABB::new([-6.0, -6.0, 0.0], [-5.0, -5.0, 0.0])];

    assert_eq!(frustum.intersects(&aabb_array[0]), IntersectionResult::Intersects);
    assert_eq!(frustum.intersects(&aabb_array[1]), IntersectionResult::Intersects);
}

// gtest: BoundingVolumes.AabbIntersectsFrustumEdgeCase
#[test]
fn aabb_intersects_frustum_edge_case() {
    let frustum = create_test_frustum(PI / 2.0, 1.0, 0.1, 100.0, -5.0, PI / 4.0);
    let aabb = AABB::new([-10.0, 10.0, 0.0], [10.0, 12.0, 0.0]);

    // Intersection test should report intersection even though shapes are separate
    assert_eq!(frustum.intersects(&aabb), IntersectionResult::Intersects);
    assert_eq!(frustum.intersects_precise(&aabb, false), IntersectionResult::Separate);
}
