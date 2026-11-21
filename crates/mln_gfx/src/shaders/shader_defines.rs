//! The buffer ids the engine reserves, the bounds of the ranges a
//! program's own ids fall in, and how many buffers, textures and vertex
//! attributes a program can have. [`Program`](crate::shaders::program::Program)
//! says which ids a program's buffers take.

/// The frame's uniform block
/// ([`GlobalPaintParamsUBO`](crate::shaders::layer_ubo::GlobalPaintParamsUBO)).
pub const ID_GLOBAL_PAINT_PARAMS_UBO: usize = 0;
/// The draw's index into the storage buffers it reads, which the backend
/// binds.
pub const ID_GLOBAL_UBO_INDEX: usize = 1;
/// How many ids the frame's buffers take.
pub const GLOBAL_UBO_COUNT: usize = 2;

/// The first id of the storage buffers a layer's draws share, an element of
/// each per draw.
pub const LAYER_SSBO_START_ID: usize = GLOBAL_UBO_COUNT;
/// A storage buffer that only a program's vertex stage reads.
pub const ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO: usize = LAYER_SSBO_START_ID;
/// A storage buffer that only a program's fragment stage reads.
pub const ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO: usize = LAYER_SSBO_START_ID + 1;
/// The per-draw projection block
/// ([`ProjectionUBO`](crate::shaders::layer_ubo::ProjectionUBO)), a storage
/// buffer.
pub const ID_PROJECTION_UBO: usize = LAYER_SSBO_START_ID + 2;
/// The first id after the reserved ones.
pub const DRAWABLE_RESERVED_UBO_COUNT: usize = LAYER_SSBO_START_ID + 3;
/// The end of the buffer ids of a program, but for the vertex buffers it
/// reads as storage buffers.
pub const MAX_UBO_COUNT_PER_SHADER: usize = 7;
/// How many textures a program can sample.
pub const MAX_TEXTURE_COUNT_PER_SHADER: usize = 3;
/// How many vertex attributes a program can read.
pub const MAX_ATTRIBUTE_COUNT_PER_SHADER: usize = 12;
