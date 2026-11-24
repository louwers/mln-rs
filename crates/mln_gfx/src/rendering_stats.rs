/// Counters of the rendering backend. The `num_*` and `mem_*` counters of
/// buffers, textures and frame buffers count the live resources; `total_*`,
/// `*_updates` and `*_bytes` grow over the context's life.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderingStats {
    /// Frame CPU encoding time (seconds).
    pub encoding_time: f64,
    /// Frame CPU rendering time (seconds).
    pub rendering_time: f64,
    /// Number of frames rendered.
    pub num_frames: i32,
    /// Draw calls executed during the most recent frame.
    pub num_draw_calls: i32,
    /// Draw calls executed during all the frames.
    pub total_draw_calls: i32,
    /// Textures created.
    pub num_created_textures: i32,
    /// Live textures.
    pub num_active_textures: i32,
    /// Net texture bindings.
    pub num_texture_bindings: i32,
    /// Times a texture was updated.
    pub num_texture_updates: i32,
    /// Bytes written in texture updates.
    pub texture_update_bytes: usize,
    /// Buffers created.
    pub total_buffers: usize,
    /// Backend buffer objects created.
    pub total_buffer_objs: usize,
    /// Times a buffer was updated.
    pub buffer_updates: usize,
    /// Times a backend buffer object was updated.
    pub buffer_obj_updates: usize,
    /// Bytes written in buffer updates.
    pub buffer_update_bytes: usize,
    /// Live buffers of every kind.
    pub num_buffers: i32,
    /// Live offscreen frame buffers.
    pub num_frame_buffers: i32,
    /// Live index buffers.
    pub num_index_buffers: i32,
    /// Bytes written in index buffer updates.
    pub index_update_bytes: usize,
    /// Live vertex buffers.
    pub num_vertex_buffers: i32,
    /// Bytes written in vertex buffer updates.
    pub vertex_update_bytes: usize,
    /// Live uniform and storage buffers.
    pub num_uniform_buffers: i32,
    /// Times a uniform or storage buffer was updated.
    pub num_uniform_updates: i32,
    /// Bytes written in uniform and storage buffer updates.
    pub uniform_update_bytes: usize,
    /// Bytes of the live textures.
    pub mem_textures: i32,
    /// Bytes of the live buffers of every kind.
    pub mem_buffers: i32,
    /// Bytes of the live index buffers.
    pub mem_index_buffers: i32,
    /// Bytes of the live vertex buffers.
    pub mem_vertex_buffers: i32,
    /// Bytes of the live uniform and storage buffers.
    pub mem_uniform_buffers: i32,
    /// Stencil buffer clears.
    pub stencil_clears: i32,
    /// Stencil buffer updates.
    pub stencil_updates: i32,
    /// What the drapes of 3D terrain hold, and what the most recent frame
    /// drew of them.
    pub terrain: TerrainStats,
}

/// What the drapes of 3D terrain hold after the most recent frame, and what
/// that frame drew of them: all zero for a frame without terrain. Its byte
/// counts are of what the drapes hold now, as the `mem_*` counters of
/// [`RenderingStats`] are; a depth and stencil texture counts 4 bytes a
/// texel, though a backend may store it in more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainStats {
    /// Drapes whose textures are made: one for each terrain tile and stack
    /// whose layers draw something there.
    pub drapes: usize,
    /// Drapes drawn during the most recent frame, each followed by the
    /// generation of its mips. The others kept what they last drew.
    pub drapes_drawn: usize,
    /// Bytes of the drapes' textures, their mips included, and of the depth
    /// and stencil and composite textures they draw with.
    pub mem_drapes: usize,
}
