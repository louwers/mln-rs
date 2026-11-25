//! The ids that draws bind buffers at, and how many buffers, textures and
//! vertex attributes a program can have. A program names its buffers by
//! their [`ProgramBuffer`](crate::shaders::program::ProgramBuffer)s, whose
//! constructors give them their ids, and the engine's by theirs
//! ([`PAINT_PARAMS`](crate::shaders::program::PAINT_PARAMS),
//! [`PROJECTION`](crate::shaders::program::PROJECTION)): the ids here are
//! the layout that those constructors and the backends' bindings share.

/// The paint parameters' block
/// ([`GlobalPaintParamsUBO`](crate::shaders::layer_ubo::GlobalPaintParamsUBO)),
/// which the renderer binds for every draw.
pub const PAINT_PARAMS_ID: usize = 0;
/// The draw's index into the storage buffers it reads, `globalIndex`
/// ([`Draw::global_index`](crate::gpu::Draw::global_index)), which the
/// backend binds.
pub const GLOBAL_INDEX_ID: usize = 1;
/// A program's buffer of each draw's values that only its vertex stage
/// reads.
pub const VERTEX_DRAW_ID: usize = 2;
/// A program's buffer of each draw's values that only its fragment stage
/// reads.
pub const FRAGMENT_DRAW_ID: usize = 3;
/// A draw's projection block
/// ([`ProjectionUBO`](crate::shaders::layer_ubo::ProjectionUBO)).
pub const PROJECTION_ID: usize = 4;
/// The id of a program's uniform block at slot 0, and of its buffer of each
/// draw's values that both stages read; slot 1's is the next.
pub const FIRST_UNIFORM_ID: usize = 5;
/// How many uniform block slots a program has.
pub const UNIFORM_SLOTS: usize = 2;
/// The uniform and storage buffer ids, from 0: the engine's and a
/// program's blocks.
pub const BUFFER_IDS: usize = FIRST_UNIFORM_ID + UNIFORM_SLOTS;
/// The id of a program's first vertex buffer that its vertex stage reads as
/// a storage buffer, after the blocks' ids.
pub const FIRST_VERTICES_ID: usize = BUFFER_IDS;
/// How many vertex buffers a program's vertex stage can read as storage
/// buffers, at the ids from [`FIRST_VERTICES_ID`].
pub const VERTICES_SLOTS: usize = 3;
/// How many textures a program can sample.
pub const MAX_TEXTURES: usize = 3;
/// How many vertex attributes a program can read.
pub const MAX_ATTRIBUTES: usize = 12;
