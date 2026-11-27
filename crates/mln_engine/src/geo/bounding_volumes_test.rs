use std::f64::consts::PI;

use glam::dcamera::rh::proj::opengl::perspective;
use glam::{DMat4, DVec3, DVec4, dvec3};

use super::*;

// gtest: BoundingVolumes.CreateAabb
#[test]
fn create_aabb() {
    let aabb = Aabb::new(dvec3(0.0, 0.0, 0.0), dvec3(2.0, 4.0, 6.0));

    assert_eq!(aabb.min, dvec3(0.0, 0.0, 0.0));
    assert_eq!(aabb.max, dvec3(2.0, 4.0, 6.0));
}

// gtest: BoundingVolumes.AabbEquality
#[test]
fn aabb_equality() {
    let aabb0 = Aabb::new(dvec3(-4.23, 1.01, 2.0), dvec3(-1.0, 1.02, 100.0));
    let aabb1 = Aabb::new(dvec3(-4.23, 1.01, 2.0), dvec3(-1.0, 1.02, 100.0));
    let aabb2 = Aabb::new(dvec3(-4.23, 1.01, 2.0), dvec3(-1.0, 1.03, 55.0));

    assert_eq!(aabb0, aabb1);
    assert_ne!(aabb0, aabb2);
}

// gtest: BoundingVolumes.CreateAabbQuadrants
#[test]
fn create_aabb_quadrants() {
    let aabb = Aabb::new(dvec3(0.0, 0.0, 0.0), dvec3(2.0, 4.0, 16.0));

    let quadrant0 = Aabb::new(dvec3(0.0, 0.0, 0.0), dvec3(1.0, 2.0, 16.0));
    let quadrant1 = Aabb::new(dvec3(1.0, 0.0, 0.0), dvec3(2.0, 2.0, 16.0));
    let quadrant2 = Aabb::new(dvec3(0.0, 2.0, 0.0), dvec3(1.0, 4.0, 16.0));
    let quadrant3 = Aabb::new(dvec3(1.0, 2.0, 0.0), dvec3(2.0, 4.0, 16.0));

    assert_eq!(aabb.quadrant(0), quadrant0);
    assert_eq!(aabb.quadrant(1), quadrant1);
    assert_eq!(aabb.quadrant(2), quadrant2);
    assert_eq!(aabb.quadrant(3), quadrant3);
}

// gtest: BoundingVolumes.AabbDistanceToPoint
#[test]
fn aabb_distance_to_point() {
    let aabb = Aabb::new(dvec3(-1.0, -1.0, -1.0), dvec3(1.0, 1.0, 1.0));

    assert_eq!(aabb.distance_xyz(dvec3(0.5, -0.5, 0.5)), dvec3(0.0, 0.0, 0.0));
    assert_eq!(aabb.distance_xyz(dvec3(1.0, 1.0, 1.0)), dvec3(0.0, 0.0, 0.0));
    assert_eq!(aabb.distance_xyz(dvec3(0.0, 10.0, 0.0)), dvec3(0.0, 9.0, 0.0));
    assert_eq!(aabb.distance_xyz(dvec3(-2.0, -2.0, -4.0)), dvec3(1.0, 1.0, 3.0));
}

// gtest: BoundingVolumes.AabbAabbIntersection
#[test]
fn aabb_aabb_intersection() {
    let aabb0 = Aabb::new(dvec3(1.0, 0.0, 0.0), dvec3(3.0, 2.0, 1.0));
    let aabb1 = Aabb::new(dvec3(2.5, 1.5, -1.0), dvec3(6.0, 2.5, 1.0));
    let aabb2 = Aabb::new(dvec3(3.5, -2.0, -2.0), dvec3(5.0, 1.0, -1.5));
    let aabb3 = Aabb::new(dvec3(0.5, -1.0, -1.0), dvec3(6.5, 3.0, 4.0));

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

    let aabb_array: [Aabb; 3] = [
        Aabb::new(dvec3(-1.0, -1.0, 0.0), dvec3(1.0, 1.0, 0.0)),
        Aabb::new(dvec3(-5.0, -5.0, 0.0), dvec3(5.0, 5.0, 0.0)),
        Aabb::new(dvec3(-5.0, -5.0, 0.0), dvec3(-4.0, -2.0, 0.0)),
    ];

    assert_eq!(frustum.intersects(&aabb_array[0]), IntersectionResult::Contains);
    assert_eq!(frustum.intersects(&aabb_array[1]), IntersectionResult::Contains);
    assert_eq!(frustum.intersects(&aabb_array[2]), IntersectionResult::Contains);
}

// gtest: BoundingVolumes.AabbIntersectsFrustum
#[test]
fn aabb_intersects_frustum() {
    let frustum = create_test_frustum(PI / 2.0, 1.0, 0.1, 100.0, -5.0, 0.0);

    let aabb_array: [Aabb; 2] = [
        Aabb::new(dvec3(-6.0, -6.0, 0.0), dvec3(6.0, 6.0, 0.0)),
        Aabb::new(dvec3(-6.0, -6.0, 0.0), dvec3(-4.5, -4.5, 0.0)),
    ];

    assert_eq!(frustum.intersects(&aabb_array[0]), IntersectionResult::Intersects);
    assert_eq!(frustum.intersects(&aabb_array[1]), IntersectionResult::Intersects);
}

#[test]
fn aabb_touching_the_frustum_is_separate_from_it() {
    let frustum = create_test_frustum(PI / 2.0, 1.0, 0.1, 100.0, -5.0, 0.0);

    // At a corner and along an edge, from outside.
    let corner = Aabb::new(dvec3(-6.0, -6.0, 0.0), dvec3(-5.0, -5.0, 0.0));
    let edge = Aabb::new(dvec3(5.0, -1.0, 0.0), dvec3(6.0, 1.0, 0.0));
    assert_eq!(frustum.intersects(&corner), IntersectionResult::Separate);
    assert_eq!(frustum.intersects(&edge), IntersectionResult::Separate);
    // From inside, the frustum still contains it.
    let inside = Aabb::new(dvec3(4.0, -1.0, 0.0), dvec3(5.0, 1.0, 0.0));
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

    let overlapping = Aabb::new(dvec3(x + 5.0 - 1.0 / 512.0, -1.0, 0.0), dvec3(x + 6.0, 1.0, 0.0));
    let touching = Aabb::new(dvec3(x + 5.0, -1.0, 0.0), dvec3(x + 6.0, 1.0, 0.0));
    assert_eq!(frustum.intersects(&overlapping), IntersectionResult::Intersects);
    assert_eq!(frustum.intersects(&touching), IntersectionResult::Separate);
}

// gtest: BoundingVolumes.AabbIntersectsFrustumEdgeCase
#[test]
fn aabb_intersects_frustum_edge_case() {
    let frustum = create_test_frustum(PI / 2.0, 1.0, 0.1, 100.0, -5.0, PI / 4.0);
    let aabb = Aabb::new(dvec3(-10.0, 10.0, 0.0), dvec3(10.0, 12.0, 0.0));

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

/// A map's projection, as a `View` builds it: from world pixels of a world
/// `world_size` wide, with z in meters at `pixels_per_meter`, to clip space,
/// for a camera `camera_to_center` pixels from the center, pitched toward
/// -y on screen.
fn map_projection(world_size: f64, pixels_per_meter: f64, pitch: f64, bearing: f64) -> DMat4 {
    let camera_to_center = 500.0;
    perspective(PI / 4.0, 1.0, 1.0, 4000.0)
        * DMat4::from_scale(dvec3(1.0, -1.0, 1.0))
        * DMat4::from_translation(dvec3(0.0, 0.0, -camera_to_center))
        * DMat4::from_rotation_x(pitch)
        * DMat4::from_rotation_z(bearing)
        * DMat4::from_translation(dvec3(-world_size / 2.0, -world_size / 2.0, 0.0))
        * DMat4::from_scale(dvec3(1.0, 1.0, pixels_per_meter))
}

#[test]
fn metres_frustum_agrees_with_the_tile_unit_frustum_on_flat_boxes() {
    let world_size = 512.0 * 2.0f64.powi(14);
    for (pitch, bearing) in [(0.0, 0.0), (1.0, 0.3), (1.3, -2.0)] {
        let inv_proj = map_projection(world_size, 0.25, pitch, bearing).try_inverse().unwrap();
        let tiles = Frustum::from_inv_proj_matrix(&inv_proj, world_size, 14.0, false, None);
        let metres = Frustum::from_inv_proj_matrix_metres(&inv_proj, world_size, 14.0, false);
        let center = 8192.0;
        let (mut separate, mut contained) = (0, 0);
        // Boxes a fraction off the tile grid, so none touches a plane.
        for i in -12..12 {
            for j in -12..12 {
                let min = dvec3(center + f64::from(i) * 0.37 + 0.011, center + f64::from(j) * 0.41 + 0.013, 0.0);
                let aabb = Aabb::new(min, min + dvec3(0.3, 0.3, 0.0));
                let result = tiles.intersects(&aabb);
                assert_eq!(result, metres.intersects_box(&aabb), "pitch {pitch}, box {aabb:?}");
                separate += usize::from(result == IntersectionResult::Separate);
                contained += usize::from(result == IntersectionResult::Contains);
            }
        }
        assert!(separate > 0 && contained > 0, "pitch {pitch}: {separate} separate, {contained} contained");
    }
}

#[test]
fn raised_box_below_the_bottom_edge_is_kept_and_raising_does_not_keep_one_past_the_top() {
    let world_size = 512.0 * 2.0f64.powi(14);
    let pixels_per_meter = 0.25;
    let pitch = 1.0;
    let inv_proj = map_projection(world_size, pixels_per_meter, pitch, 0.0).try_inverse().unwrap();
    let frustum = Frustum::from_inv_proj_matrix_metres(&inv_proj, world_size, 14.0, false);
    let center = 8192.0;
    // The camera's height in meters, and the side of the center it is on,
    // where the bottom edge of the view is.
    let camera_height = 500.0 * pitch.cos() / pixels_per_meter;
    let near_y = frustum.points()[..4].iter().map(|point| point.y).sum::<f64>() / 4.0;
    let toward_camera = (near_y - center).signum();

    // Boxes a tenth of a tile long in a strip along the line of sight, from
    // `depth` up to `height` meters.
    let strip = |step: i32, depth: f64, height: f64| {
        let y = center + toward_camera * f64::from(step) * 0.1;
        Aabb::new(
            dvec3(center - 0.05, y.min(y + toward_camera * 0.1), depth),
            dvec3(center + 0.05, y.max(y + toward_camera * 0.1), height),
        )
    };
    let visible = |aabb: Aabb| frustum.intersects_box(&aabb) != IntersectionResult::Separate;
    assert!(visible(strip(0, 0.0, 0.0)));
    let bottom = (0..100).find(|&step| !visible(strip(step, 0.0, 0.0))).expect("a box below the bottom edge");
    let top =
        (0..100).map(|step| -step).find(|&step| !visible(strip(step, 0.0, 0.0))).expect("a box past the top edge");

    // Raised ground rises into view from below the bottom edge; ground as
    // far below sea level stays out.
    let height = camera_height * 0.8;
    assert!(visible(strip(bottom, 0.0, height)));
    assert!(!visible(strip(bottom, -height, 0.0)));
    // Past the top edge, raised ground only climbs further out of view.
    for step in (top - 20)..=top {
        assert!(!visible(strip(step, 0.0, height)), "step {step}");
    }
    // Ground above the camera is behind it.
    assert!(!visible(strip(0, camera_height * 1.5, camera_height * 2.0)));
}
