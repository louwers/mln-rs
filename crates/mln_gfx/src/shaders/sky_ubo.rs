use crate::shaders::block::uniform_block;

uniform_block! {
    /// The sky properties uniform block. The colors are premultiplied
    /// r, g, b, a floats.
    pub struct SkyPropsUBO {
        pub sky_color: [f32; 4],
        pub horizon_color: [f32; 4],
        pub horizon: [f32; 2],
        pub horizon_normal: [f32; 2],
        pub viewport_size: [f32; 2],
        pub sky_horizon_blend: f32,
        pub sky_blend: f32,
    }
}

uniform_block! {
    /// The atmosphere properties uniform block.
    pub struct AtmospherePropsUBO {
        pub inv_view_projection: [f32; 4 * 4],
        pub camera_position: [f32; 4],
        /// The fourth component stores the evaluated atmosphere blend.
        pub sun_position: [f32; 4],
    }
}
