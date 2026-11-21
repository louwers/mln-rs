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
    /// Render passes begun during the most recent frame, those of the
    /// layers' own targets and of the terrain's drapes included.
    pub num_render_passes: i32,
    /// Textures whose mip levels the most recent frame generated.
    pub num_mipmap_generations: i32,
    /// What 3D terrain holds, and what it drew in the most recent frame.
    pub terrain: TerrainStats,
}

/// What 3D terrain holds after the most recent frame, and what that frame
/// drew of it: all zero for a frame without terrain. Its byte counts are
/// of what the terrain holds now, as the `mem_*` counters of
/// [`RenderingStats`] are; a depth and stencil texture counts 4 bytes a
/// texel, though a backend may store it in more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainStats {
    /// Stacks of draped layers, each of which has a drape of every terrain
    /// tile.
    pub stacks: usize,
    /// Drapes whose textures are made: one for each terrain tile and stack.
    pub drapes: usize,
    /// Drapes drawn during the most recent frame, each followed by the
    /// generation of its mips.
    pub drapes_drawn: usize,
    /// Render passes of the drapes drawn during the most recent frame,
    /// among [`RenderingStats::num_render_passes`].
    pub drape_passes: usize,
    /// Draw calls into the drapes during the most recent frame, among
    /// [`RenderingStats::num_draw_calls`].
    pub drape_draw_calls: usize,
    /// Draw calls of the terrain's surface during the most recent frame,
    /// among [`RenderingStats::num_draw_calls`].
    pub surface_draw_calls: usize,
    /// Bytes of the drapes' textures, their mips included, which
    /// [`RenderingStats::mem_textures`] counts too. Every stack's drapes
    /// take an equal share.
    pub mem_drapes: usize,
    /// Bytes of the depth and stencil texture that the drapes draw with in
    /// turn, which [`RenderingStats::mem_textures`] doesn't count.
    pub mem_drape_depth_stencil: usize,
    /// Bytes of the textures of the composite that the drapes draw their
    /// translucent and blended layers with: its color textures, which
    /// [`RenderingStats::mem_textures`] counts too, and its depth and
    /// stencil, which it doesn't.
    pub mem_drape_composite: usize,
    /// Bytes of the DEM textures that the surface reads, which
    /// [`RenderingStats::mem_textures`] counts too.
    pub mem_dem_textures: usize,
    /// Bytes of the DEM images that the ground the map's camera stands on
    /// holds, each once, which the terrain source's tiles share while they
    /// hold them.
    pub mem_dem_data: usize,
    /// Bytes of the table of the DEM tiles' remembered height ranges.
    pub mem_height_ranges: usize,
}
