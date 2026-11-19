use std::f64::consts::PI;

use glam::dcamera::rh::proj::opengl::perspective;
use glam::{DMat4, DVec3, DVec4, dvec3};

use super::*;

// gtest: BoundingVolumes.CreateAabb
#[test]
fn create_aabb() {
    let aabb = AABB::new(dvec3(0.0, 0.0, 0.0), dvec3(2.0, 4.0, 6.0));

    assert_eq!(aabb.min, dvec3(0.0, 0.0, 0.0));
    assert_eq!(aabb.max, dvec3(2.0, 4.0, 6.0));
}

// gtest: BoundingVolumes.AabbEquality
#[test]
fn aabb_equality() {
    let aabb0 = AABB::new(dvec3(-4.23, 1.01, 2.0), dvec3(-1.0, 1.02, 100.0));
    let aabb1 = AABB::new(dvec3(-4.23, 1.01, 2.0), dvec3(-1.0, 1.02, 100.0));
    let aabb2 = AABB::new(dvec3(-4.23, 1.01, 2.0), dvec3(-1.0, 1.03, 55.0));

    assert_eq!(aabb0, aabb1);
    assert_ne!(aabb0, aabb2);
}

// gtest: BoundingVolumes.CreateAabbQuadrants
#[test]
fn create_aabb_quadrants() {
    let aabb = AABB::new(dvec3(0.0, 0.0, 0.0), dvec3(2.0, 4.0, 16.0));

    let quadrant0 = AABB::new(dvec3(0.0, 0.0, 0.0), dvec3(1.0, 2.0, 16.0));
    let quadrant1 = AABB::new(dvec3(1.0, 0.0, 0.0), dvec3(2.0, 2.0, 16.0));
    let quadrant2 = AABB::new(dvec3(0.0, 2.0, 0.0), dvec3(1.0, 4.0, 16.0));
    let quadrant3 = AABB::new(dvec3(1.0, 2.0, 0.0), dvec3(2.0, 4.0, 16.0));

    assert_eq!(aabb.quadrant(0), quadrant0);
    assert_eq!(aabb.quadrant(1), quadrant1);
    assert_eq!(aabb.quadrant(2), quadrant2);
    assert_eq!(aabb.quadrant(3), quadrant3);
}

// gtest: BoundingVolumes.AabbDistanceToPoint
#[test]
fn aabb_distance_to_point() {
    let aabb = AABB::new(dvec3(-1.0, -1.0, -1.0), dvec3(1.0, 1.0, 1.0));

    assert_eq!(aabb.distance_xyz(dvec3(0.5, -0.5, 0.5)), dvec3(0.0, 0.0, 0.0));
    assert_eq!(aabb.distance_xyz(dvec3(1.0, 1.0, 1.0)), dvec3(0.0, 0.0, 0.0));
    assert_eq!(aabb.distance_xyz(dvec3(0.0, 10.0, 0.0)), dvec3(0.0, 9.0, 0.0));
    assert_eq!(aabb.distance_xyz(dvec3(-2.0, -2.0, -4.0)), dvec3(1.0, 1.0, 3.0));
}

// gtest: BoundingVolumes.AabbAabbIntersection
#[test]
fn aabb_aabb_intersection() {
    let aabb0 = AABB::new(dvec3(1.0, 0.0, 0.0), dvec3(3.0, 2.0, 1.0));
    let aabb1 = AABB::new(dvec3(2.5, 1.5, -1.0), dvec3(6.0, 2.5, 1.0));
    let aabb2 = AABB::new(dvec3(3.5, -2.0, -2.0), dvec3(5.0, 1.0, -1.5));
    let aabb3 = AABB::new(dvec3(0.5, -1.0, -1.0), dvec3(6.5, 3.0, 4.0));

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
    let inv_proj_matrix = perspective(PI / 2.0, 1.0, 0.1, 100.0).try_inverse().unwrap();

    let frustum = Frustum::from_inv_proj_matrix(&inv_proj_matrix, 1.0, 0.0, false, None);

    let expected_points: [DVec3; 8] = [
        dvec3(-0.1, 0.1, -0.1),
        dvec3(0.1, 0.1, -0.1),
        dvec3(0.1, -0.1, -0.1),
        dvec3(-0.1, -0.1, -0.1),
        dvec3(-100.0, 100.0, -100.0),
        dvec3(100.0, 100.0, -100.0),
        dvec3(100.0, -100.0, -100.0),
        dvec3(-100.0, -100.0, -100.0),
    ];

    let expected_planes: [DVec4; 6] = [
        DVec4::new(0.0, -0.707, 0.707, 0.0),
        DVec4::new(-0.707, 0.0, 0.707, 0.0),
        DVec4::new(0.707, 0.0, 0.707, 0.0),
        DVec4::new(0.0, 0.707, 0.707, 0.0),
        DVec4::new(0.0, 0.0, 1.0, 0.1),
        DVec4::new(0.0, 0.0, -1.0, -100.0),
    ];

    // C `round`: half away from zero, like f64::round.
    let actual_points = frustum.points().map(|v| (v * 10.0).round() / 10.0);
    let actual_planes = frustum.planes().map(|v| (v * 1000.0).round() / 1000.0);

    assert_eq!(actual_points, expected_points);
    assert_eq!(actual_planes, expected_planes);
}

fn create_test_frustum(fovy: f64, aspect_ratio: f64, z_near: f64, z_far: f64, elevation: f64, bearing: f64) -> Frustum {
    // A complete projection matrix from world space to clip space.
    let proj = perspective(fovy, aspect_ratio, z_near, z_far)
        * DMat4::from_scale(dvec3(1.0, -1.0, 1.0))
        * DMat4::from_translation(dvec3(0.0, 0.0, elevation))
        * DMat4::from_rotation_z(bearing);

    Frustum::from_inv_proj_matrix(&proj.try_inverse().unwrap(), 1.0, 0.0, false, None)
}

// gtest: BoundingVolumes.AabbFullyInsideFrustum
#[test]
fn aabb_fully_inside_frustum() {
    let frustum = create_test_frustum(PI / 2.0, 1.0, 0.1, 100.0, -5.0, 0.0);

    let aabb_array: [AABB; 3] = [
        AABB::new(dvec3(-1.0, -1.0, 0.0), dvec3(1.0, 1.0, 0.0)),
        AABB::new(dvec3(-5.0, -5.0, 0.0), dvec3(5.0, 5.0, 0.0)),
        AABB::new(dvec3(-5.0, -5.0, 0.0), dvec3(-4.0, -2.0, 0.0)),
    ];

    assert_eq!(frustum.intersects(&aabb_array[0]), IntersectionResult::Contains);
    assert_eq!(frustum.intersects(&aabb_array[1]), IntersectionResult::Contains);
    assert_eq!(frustum.intersects(&aabb_array[2]), IntersectionResult::Contains);
}

// gtest: BoundingVolumes.AabbIntersectsFrustum
#[test]
fn aabb_intersects_frustum() {
    let frustum = create_test_frustum(PI / 2.0, 1.0, 0.1, 100.0, -5.0, 0.0);

    let aabb_array: [AABB; 2] = [
        AABB::new(dvec3(-6.0, -6.0, 0.0), dvec3(6.0, 6.0, 0.0)),
        AABB::new(dvec3(-6.0, -6.0, 0.0), dvec3(-4.5, -4.5, 0.0)),
    ];

    assert_eq!(frustum.intersects(&aabb_array[0]), IntersectionResult::Intersects);
    assert_eq!(frustum.intersects(&aabb_array[1]), IntersectionResult::Intersects);
}

#[test]
fn aabb_touching_the_frustum_is_separate_from_it() {
    let frustum = create_test_frustum(PI / 2.0, 1.0, 0.1, 100.0, -5.0, 0.0);

    // At a corner and along an edge, from outside.
    let corner = AABB::new(dvec3(-6.0, -6.0, 0.0), dvec3(-5.0, -5.0, 0.0));
    let edge = AABB::new(dvec3(5.0, -1.0, 0.0), dvec3(6.0, 1.0, 0.0));
    assert_eq!(frustum.intersects(&corner), IntersectionResult::Separate);
    assert_eq!(frustum.intersects(&edge), IntersectionResult::Separate);
    // From inside, the frustum still contains it.
    let inside = AABB::new(dvec3(4.0, -1.0, 0.0), dvec3(5.0, 1.0, 0.0));
    assert_eq!(frustum.intersects(&inside), IntersectionResult::Contains);
}

#[test]
fn aabb_one_pixel_inside_the_frustum_far_from_the_origin_intersects_it() {
    // In tile units at zoom 25 near the antimeridian, where a pixel is
    // 1/512: the frustum's footprint spans x ± 5.
    let x = f64::from(1u32 << 25);
    let proj = perspective(PI / 2.0, 1.0, 0.1, 100.0)
        * DMat4::from_scale(dvec3(1.0, -1.0, 1.0))
        * DMat4::from_translation(dvec3(-x, 0.0, -5.0));
    let frustum = Frustum::from_inv_proj_matrix(&proj.try_inverse().unwrap(), 1.0, 0.0, false, None);

    let overlapping = AABB::new(dvec3(x + 5.0 - 1.0 / 512.0, -1.0, 0.0), dvec3(x + 6.0, 1.0, 0.0));
    let touching = AABB::new(dvec3(x + 5.0, -1.0, 0.0), dvec3(x + 6.0, 1.0, 0.0));
    assert_eq!(frustum.intersects(&overlapping), IntersectionResult::Intersects);
    assert_eq!(frustum.intersects(&touching), IntersectionResult::Separate);
}

// gtest: BoundingVolumes.AabbIntersectsFrustumEdgeCase
#[test]
fn aabb_intersects_frustum_edge_case() {
    let frustum = create_test_frustum(PI / 2.0, 1.0, 0.1, 100.0, -5.0, PI / 4.0);
    let aabb = AABB::new(dvec3(-10.0, 10.0, 0.0), dvec3(10.0, 12.0, 0.0));

    // Intersection test should report intersection even though shapes are separate
    assert_eq!(frustum.intersects(&aabb), IntersectionResult::Intersects);
    assert_eq!(frustum.intersects_precise(&aabb, false), IntersectionResult::Separate);
}

#[test]
fn pushing_the_far_plane_moves_the_far_corners_along_their_rays() {
    // A camera at the origin looking down -Z, its far plane at Z = -100.
    let proj = perspective(PI / 2.0, 1.0, 0.1, 100.0) * DMat4::from_scale(dvec3(1.0, -1.0, 1.0));
    let frustum = Frustum::from_inv_proj_matrix(&proj.try_inverse().unwrap(), 1.0, 0.0, false, None);
    let far_side = |frustum: &Frustum| frustum.planes()[5].dot(DVec4::new(0.0, 0.0, -105.0, 1.0));
    assert!(far_side(&frustum) < 0.0);

    let pushed = frustum.with_far_plane_pushed(10.0, DVec3::ZERO);

    for (before, after) in frustum.points()[4..].iter().zip(&pushed.points()[4..]) {
        assert!(after.abs_diff_eq(*before * 1.1, 1e-6), "{before:?} -> {after:?}");
    }
    assert_eq!(&frustum.points()[..4], &pushed.points()[..4]);
    // A point 5 past the old far plane is inside the new one.
    assert!(far_side(&pushed) > 0.0);
}
