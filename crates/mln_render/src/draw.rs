//! What a layer draws with: the other half of the [layer
//! contract](crate::layer).
//!
//! A layer makes GPU resources in the renderer's [`Resources`] and keeps
//! their handles: [`Buffer`]s of vertices or [`IndexBuffer`]s, [`Texture`]s,
//! and [`Mesh`]es, each a program bound to indexes, buffers and textures
//! with the state it draws in ([`MeshDesc`]), in the frame's projection. A
//! tile's surface ([`Resources::tile_surface`]) is a shared buffer of the
//! engine's. A bucket's buffers take in place what changes in its data
//! (feature state, placement), and meshes over them see it. An entry of the
//! resources goes once its last handle drops and the GPU is done with it,
//! and a mesh holds what it was made from.
//!
//! Each frame, a layer pushes its draws to [`Draws`]: a [`Draw`] of one of
//! its meshes, placed in the layer's order, with the values of its
//! program's blocks of each draw ([`DrawBlocks`]) and, for a draw of a tile,
//! the tile's projection. The engine writes what the blocks hold of a mesh's
//! paint values ([`MeshDesc::paint`]), sorts each [`Group`] by the draws'
//! order, writes each block once for all of the group's draws, places the
//! group in the layers' order and depth, and draws it in the frame's passes,
//! as the draws' [`Depth`] and clipping say, onto the frame or as one
//! surface ([`Composite`]).
//!
//! The resources and their handles belong to the renderer's thread: they
//! are neither `Send` nor `Sync`.

pub use mln_gfx::shaders::block::DrawBlocks;

pub use crate::draw_objects::Mesh;
pub use crate::encode::{Depth, Draw, Draws, Group};
pub use crate::gfx::context::Resources;
pub use crate::gfx::texture2d::TextureMut;
pub use crate::handles::{Buffer, IndexBuffer, Texture, TileSurface, Vertex};
pub use crate::layer_composite::Composite;
pub use crate::mesh_desc::MeshDesc;
