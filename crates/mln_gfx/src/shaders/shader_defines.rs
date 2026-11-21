//! The buffer ids the engine reserves, and how many buffers, textures and
//! vertex attributes a program can have. Every other id is its program's:
//! a program's buffers take the ids from [`DRAWABLE_RESERVED_UBO_COUNT`] to
//! [`MAX_UBO_COUNT_PER_SHADER`], and the vertex buffers it reads as storage
//! buffers the ids after those.

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
/// The end of the ids a layer's storage buffers can have, from
/// [`LAYER_SSBO_START_ID`]. A layer's uniform buffers have the ids from
/// [`DRAWABLE_RESERVED_UBO_COUNT`], so that id can be either.
pub const LAYER_UBO_START_ID: usize = DRAWABLE_RESERVED_UBO_COUNT + 1;

/// The end of the buffer ids of a program, but for the vertex buffers it
/// reads as storage buffers.
pub const MAX_UBO_COUNT_PER_SHADER: usize = 7;
/// How many textures a program can sample.
pub const MAX_TEXTURE_COUNT_PER_SHADER: usize = 3;
/// How many vertex attributes a program can read.
pub const MAX_ATTRIBUTE_COUNT_PER_SHADER: usize = 12;
