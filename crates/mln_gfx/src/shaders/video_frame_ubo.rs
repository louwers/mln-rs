use bytemuck::{Pod, Zeroable};

/// Port-only: the video frame shader's props uniform block (112 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct VideoFramePropsUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /// The rows of the matrix that turns `(y, u, v, 1)` into RGB
    /// (`YuvColor::to_rgb_rows`).
    /* 64 */
    pub to_rgb: [[f32; 4]; 3],
    /* 112 */
}
const _: () = assert!(size_of::<VideoFramePropsUBO>() == 7 * 16);
