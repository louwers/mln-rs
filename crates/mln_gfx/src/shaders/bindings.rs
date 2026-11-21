//! The buffer ids the engine reserves, the ranges a program's own ids fall
//! in, and how many buffers, textures and vertex attributes a program can
//! have. [`Buffer`](crate::shaders::program::Buffer)'s constructors give a
//! program's buffers their ids.

/// The view's uniform block
/// ([`ViewUBO`](crate::shaders::layer_ubo::ViewUBO)).
pub const VIEW_BUFFER_ID: usize = 0;
/// The draw's index into the storage buffers it reads, which the backend
/// binds.
pub const DRAW_INDEX_ID: usize = 1;
/// The first id of a program's own buffers, after the frame's.
pub const FIRST_PROGRAM_BUFFER_ID: usize = 2;
/// A program's storage buffer that only its vertex stage reads.
pub const VERTEX_STORAGE_ID: usize = FIRST_PROGRAM_BUFFER_ID;
/// A program's storage buffer that only its fragment stage reads.
pub const FRAGMENT_STORAGE_ID: usize = FIRST_PROGRAM_BUFFER_ID + 1;
/// The per-draw projection block
/// ([`ProjectionUBO`](crate::shaders::layer_ubo::ProjectionUBO)), a storage
/// buffer.
pub const PROJECTION_BUFFER_ID: usize = FIRST_PROGRAM_BUFFER_ID + 2;
/// The first id of a program's uniform blocks, after the engine's
/// projection block.
pub const FIRST_UNIFORM_ID: usize = FIRST_PROGRAM_BUFFER_ID + 3;
/// The first id of the vertex buffers a program's vertex stage reads as
/// storage buffers, after the ids of its other buffers.
pub const FIRST_VERTEX_BUFFER_ID: usize = 7;
/// How many vertex buffers a program's vertex stage can read as storage
/// buffers, at the ids from [`FIRST_VERTEX_BUFFER_ID`].
pub const MAX_VERTEX_BUFFERS: usize = 3;
/// How many textures a program can sample.
pub const MAX_TEXTURES: usize = 3;
/// How many vertex attributes a program can read.
pub const MAX_ATTRIBUTES: usize = 12;
