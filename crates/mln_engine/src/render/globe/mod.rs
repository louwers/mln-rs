//! The globe: its tile meshes and its depth pass.

pub(crate) mod depth_pass;
pub(crate) mod globe_tile_mesh;
pub(crate) mod tile_mesh;

/// The Y coordinate of a vertex at the north pole, which the globe's vertex
/// stage maps to the pole.
pub const NORTH_POLE_Y: i16 = -32768;
/// The Y coordinate of a vertex at the south pole.
pub const SOUTH_POLE_Y: i16 = 32767;
