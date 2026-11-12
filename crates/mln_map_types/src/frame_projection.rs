//! Where a frame drew the map, for a host that draws into the frame
//! depth-tested against it. The renderer computes it
//! (`mln_renderer::frame_projection`); it lives here so that the style's
//! custom layers, which get it with each frame, can name it.

use mln_util::mat4::Mat4;

/// How a graphics API's clip space holds depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClipDepthRange {
    /// Clip z from 0 to w: Metal, Vulkan and WebGPU.
    ZeroToOne,
    /// Clip z from -w to w: OpenGL.
    NegativeOneToOne,
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
