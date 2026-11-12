//! Where a frame drew the map, for a host that draws into the frame
//! depth-tested against it.
//!
//! The renderer draws with OpenGL's projection matrices, whose clip z runs
//! from -w to w. Metal, Vulkan and WebGPU clip z below 0, so their 3D layers
//! write `depth_range_size * z / w` and drop what is nearer than the plane
//! where z is 0. OpenGL's 3D layers draw through [`zero_to_one_clip_z`] to
//! write the same depth, so the depth of a frame means one thing on every
//! backend.

use mln_gfx::backend::Type as BackendType;
use mln_util::mat4::{Mat4, matrix};
use mln_util::projection::Projection;

use crate::paint_parameters::TransformParameters;

/// How a graphics API's clip space holds depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClipDepthRange {
    /// Clip z from 0 to w: Metal, Vulkan and WebGPU.
    ZeroToOne,
    /// Clip z from -w to w: OpenGL.
    NegativeOneToOne,
}

impl ClipDepthRange {
    /// The convention of `backend`'s API.
    #[must_use]
    pub fn of(backend: BackendType) -> ClipDepthRange {
        if backend == BackendType::OpenGL { ClipDepthRange::NegativeOneToOne } else { ClipDepthRange::ZeroToOne }
    }
}

/// Where a frame drew the map, and the depth its 3D layers wrote there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameProjection {
    /// Takes Web Mercator points, x and y from 0 to 1 and z in meters, to
    /// clip coordinates in `clip_depth_range`'s convention, whose depth
    /// under the API's default depth range is the depth that the frame's 3D
    /// layers wrote at the point. Column-major.
    pub matrix: Mat4,
    pub clip_depth_range: ClipDepthRange,
    /// The distance from the camera to the plane where that depth is 0, in
    /// pixels at the camera's zoom.
    pub near_distance: f64,
    /// The distance to the plane where the depth is `far_depth`.
    pub far_distance: f64,
    /// The depth at `far_distance`, below the 2D layers' depths.
    pub far_depth: f64,
    /// Whether the frame drew a globe, which `matrix` doesn't describe.
    pub globe: bool,
}

impl FrameProjection {
    /// The projection of a frame that drew with `transform`, its 3D layers
    /// into the depth range from 0 to `depth_range_size`, on `backend`.
    #[must_use]
    pub fn new(transform: &TransformParameters, depth_range_size: f32, backend: BackendType) -> FrameProjection {
        let state = &transform.state;
        let world_size = Projection::world_size(state.get_scale());
        let mut world_to_clip: Mat4 = [0.0; 16];
        matrix::scale(&mut world_to_clip, &transform.near_clipped_proj_matrix, world_size, world_size, 1.0);
        let far_depth = f64::from(depth_range_size);
        let clip_depth_range = ClipDepthRange::of(backend);
        // Clip z over w from 0 to 1 is the 3D layers' depth over
        // `far_depth`: scale it to the depth, and on OpenGL map it to -1 to 1
        // again.
        for column in 0..4 {
            let z = world_to_clip[column * 4 + 2];
            let w = world_to_clip[column * 4 + 3];
            world_to_clip[column * 4 + 2] = match clip_depth_range {
                ClipDepthRange::ZeroToOne => far_depth * z,
                ClipDepthRange::NegativeOneToOne => 2.0 * far_depth * z - w,
            };
        }
        let near = transform.near_clipped_near_z;
        let far = state.get_far_z();
        FrameProjection {
            matrix: world_to_clip,
            clip_depth_range,
            // Where OpenGL's clip z, from -w at `near` to w at `far`, is 0.
            near_distance: 2.0 * near * far / (far + near),
            far_distance: far,
            far_depth,
            globe: state.is_globe_rendering(),
        }
    }
}

/// Makes a 3D layer's OpenGL matrix clip and write depth as Metal, Vulkan
/// and WebGPU do with it: clip z from -w to w becomes `2z - w`, so that
/// OpenGL clips what was below 0 and writes the depth range times the old z
/// over w.
pub fn zero_to_one_clip_z(matrix: &mut Mat4) {
    for column in 0..4 {
        matrix[column * 4 + 2] = 2.0 * matrix[column * 4 + 2] - matrix[column * 4 + 3];
    }
}

#[cfg(test)]
mod tests {
    use mln_map_types::{Camera, LogicalExtent, MapView, ProjectionMode, Viewport};
    use mln_style_types::ProjectionDefinition;
    use mln_util::geo::{EdgeInsets, LatLng};
    use mln_util::mat4::matrix;
    use mln_util::size::Size;

    use super::*;

    const SIZE: Size = Size { width: 300, height: 200 };

    /// A view with padding, roll, pitch, bearing and a center altitude.
    fn transform() -> TransformParameters {
        let view = MapView {
            camera: Camera {
                center: LatLng::new(40.7, -74.0).unwrap(),
                center_altitude: 30.0,
                padding: EdgeInsets::new(40.0, 60.0, 10.0, 0.0).unwrap(),
                zoom: 15.5,
                bearing: 25.0,
                pitch: 55.0,
                roll: 12.0,
                fov: 40.0,
            },
            extent: LogicalExtent::new(SIZE, 1.0),
            projection_mode: ProjectionMode::default(),
            viewport: Viewport::default(),
            projection: ProjectionDefinition::default(),
        };
        TransformParameters::new(&view.to_state().unwrap())
    }

    /// `point`, of x and y in Web Mercator's 0 to 1 and z in meters, through
    /// `matrix`: its pixel from the top left, and its clip z over w.
    fn project(matrix: &Mat4, point: [f64; 3]) -> (f64, f64, f64) {
        let mut clip = [0.0; 4];
        matrix::transform_mat4(&mut clip, &[point[0], point[1], point[2], 1.0], matrix);
        let (x, y, z) = (clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]);
        (f64::midpoint(x, 1.0) * f64::from(SIZE.width), (1.0 - y) / 2.0 * f64::from(SIZE.height), z)
    }

    fn mercator(lat_lng: &LatLng) -> [f64; 3] {
        let point = Projection::project(lat_lng, 1.0);
        let world = Projection::world_size(1.0);
        [point.x / world, point.y / world, 0.0]
    }

    #[test]
    fn the_matrix_puts_points_where_the_map_draws_them() {
        let transform = transform();
        let projection = FrameProjection::new(&transform, 0.9, BackendType::Vulkan);
        for (lat, lng) in [(40.7, -74.0), (40.703, -73.996), (40.698, -74.004), (40.705, -74.001)] {
            let lat_lng = LatLng::new(lat, lng).unwrap();
            // The state counts its screen points from the bottom.
            let mut expected = transform.state.lat_lng_to_screen_coordinate(&lat_lng);
            expected.y = f64::from(SIZE.height) - expected.y;
            let (x, y, _) = project(&projection.matrix, mercator(&lat_lng));
            assert!(
                (x - expected.x).abs() < 1e-6 && (y - expected.y).abs() < 1e-6,
                "{lat_lng:?}: {x}, {y} {expected:?}"
            );
        }
        // The padding moves the center, at its altitude, off the middle of the
        // view. The map scales meters at the camera's latitude, a little off
        // the center's.
        let [x, y, _] = mercator(&LatLng::new(40.7, -74.0).unwrap());
        let (x, y, _) = project(&projection.matrix, [x, y, 30.0]);
        assert!((x - 180.0).abs() < 1e-2 && (y - 115.0).abs() < 1e-2, "{x}, {y}");
    }

    #[test]
    fn both_conventions_give_the_depth_of_the_3d_layers() {
        let transform = transform();
        let far_depth = 0.99;
        let zero_to_one = FrameProjection::new(&transform, far_depth as f32, BackendType::Vulkan);
        let negative_one_to_one = FrameProjection::new(&transform, far_depth as f32, BackendType::OpenGL);
        assert_eq!(ClipDepthRange::ZeroToOne, zero_to_one.clip_depth_range);
        assert_eq!(ClipDepthRange::NegativeOneToOne, negative_one_to_one.clip_depth_range);
        let far_depth = zero_to_one.far_depth;
        // A 3D layer's OpenGL matrix, as Metal, Vulkan and WebGPU draw it,
        // and as OpenGL does through `zero_to_one_clip_z`.
        let world_size = Projection::world_size(transform.state.get_scale());
        let mut layer = [0.0; 16];
        matrix::scale(&mut layer, &transform.near_clipped_proj_matrix, world_size, world_size, 1.0);
        let mut gl_layer = layer;
        zero_to_one_clip_z(&mut gl_layer);
        let center = mercator(&LatLng::new(40.7, -74.0).unwrap());
        for z in [0.0, 20.0, 150.0] {
            let point = [center[0], center[1], z];
            let (_, _, layer_z) = project(&layer, point);
            let (_, _, gl_z) = project(&gl_layer, point);
            let (_, _, zero_to_one_z) = project(&zero_to_one.matrix, point);
            let (_, _, negative_z) = project(&negative_one_to_one.matrix, point);
            let written = far_depth * layer_z;
            assert!((0.0..far_depth).contains(&written), "{written}");
            assert!((far_depth * (gl_z + 1.0) / 2.0 - written).abs() < 1e-9);
            assert!((zero_to_one_z - written).abs() < 1e-9);
            assert!((f64::midpoint(negative_z, 1.0) - written).abs() < 1e-9);
        }
        // Higher points are nearer the camera, which looks down on them.
        let depth = |z| project(&zero_to_one.matrix, [center[0], center[1], z]).2;
        assert!(depth(150.0) < depth(20.0) && depth(20.0) < depth(0.0));
        assert!(zero_to_one.near_distance > 0.0 && zero_to_one.near_distance < zero_to_one.far_distance);
        assert!(!zero_to_one.globe);
    }
}
