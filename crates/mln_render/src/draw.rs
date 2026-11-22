//! What a layer draws with: the other half of the [layer
//! contract](crate::layer).
//!
//! A layer makes GPU resources in the renderer's [`Resources`] and keeps
//! their handles: [`Buffer`]s of vertices or [`IndexBuffer`]s, [`Texture`]s,
//! [`Target`]s, textures that its draws draw into, and [`Mesh`]es, each a
//! program bound to indexes, buffers of vertices or of instances, and
//! textures with the [`Sampler`]s that sample them and the state it draws
//! in ([`MeshDesc`]), in the frame's projection or, for geometry of the
//! viewport, Mercator's. A tile's surface ([`Resources::tile_surface`]) and
//! the indexes of quads ([`Resources::quads`]) are shared buffers of the
//! engine's. A bucket's
//! buffers take in place what changes in its data (feature state,
//! placement), and meshes over them see it. An entry of the resources goes
//! once its last handle drops and the GPU is done with it, and a mesh holds
//! what it was made from.
//!
//! Each frame, a layer pushes its draws to [`Draws`], in its main group, in
//! the overlay above it or into one of its targets, which draw before the
//! frame's passes: a [`Draw`] of one of its meshes, placed in the
//! layer's order, with the values of its program's blocks of each draw
//! ([`DrawBlocks`]) and, for a draw of a tile, the tile's projection. The
//! engine writes what the blocks hold of a mesh's paint values
//! ([`MeshDesc::paint`]), sorts each [`Group`] by the draws' order, writes
//! the blocks of each of the group's programs once for all of its draws,
//! places the group in the layers' order and depth, and draws it in the
//! frame's passes, as the draws' [`Depth`] and clipping say, onto the frame
//! or as one surface ([`Composite`]). A group may draw with any number of
//! programs, whose blocks differ. A layer that draws with the backend's own
//! commands asks the frame to lend them ([`Draws::lend`]).
//!
//! A style may declare named targets ([`NamedTargets`], a value of its root)
//! and draw a layer's main group and overlay into one instead of the map
//! ([`Destination`], a value of the layer), where a layer reads it
//! ([`UpdateCx::target`](crate::layer::UpdateCx::target)). The engine knows
//! no key that holds these values: an extension declares the keys
//! ([`StyleKey`](mln_style::style_key::StyleKey)), as the
//! `mln_image_process` crate does, with a layer type that reads targets. A
//! group draws into a named target as onto the map, with the masks of its
//! tiles and within the frame's scissor, but each draw over the last,
//! without the opaque pass's depth, with no composite of a blend that reads
//! what is behind it, and with no commands lent in the pass.
//!
//! The resources and their handles belong to the renderer's thread: they
//! are neither `Send` nor `Sync`.

pub use mln_gfx::shaders::block::DrawBlocks;

pub use crate::draw_objects::Mesh;
pub use crate::encode::{Depth, Draw, Draws, Group};
pub use crate::gfx::context::Resources;
pub use crate::gfx::texture2d::TextureMut;
pub use crate::handles::{Buffer, ImageVertex, IndexBuffer, Sampler, Target, Texture, TileSurface, Vertex};
pub use crate::layer_composite::Composite;
pub use crate::mesh_desc::MeshDesc;
pub use crate::named_target::{Destination, NamedTarget, NamedTargets};
