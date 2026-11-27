//! What a layer draws with: the other half of the [layer
//! contract](crate::layer).
//!
//! A layer makes its GPU objects through the [`Resources`] its hooks get and
//! keeps their handles: [`Mesh`]es, which draw a program over geometry with
//! textures, and [`Target`]s, textures of the layer's own that its draws
//! draw into ahead of the frame and that its other draws sample. An object
//! goes once its last handle drops and the GPU is done with it.
//!
//! Each frame, a layer pushes its draws into its [`Draws`]: into its main
//! [`Group`], which draws at its place in the layers' order and depth, and
//! into the groups of its targets. A [`Draw`] of a mesh says where it is
//! among the group's draws, whether it may draw in the opaque pass, how it
//! tests depth ([`Depth`]) and whether it clips to its tile, and comes with
//! the values of its program's blocks of each draw and, for a draw of a
//! tile, the tile's projection. The engine writes the blocks once for the
//! group, uploads the meshes, and draws the group in the frame's passes.
//!
//! A layer sees its frame as a [`LayerFrame`]: the map's, or a drape's. A
//! layer that draws with the backend's own commands asks the frame to lend
//! them ([`Draws::lend`]), at a time in the frame ([`LendAt`]), and gets them
//! with the frame and where it draws the map ([`Lent`]).
//!
//! The objects and their handles belong to the renderer's thread.
//!
//! Inside the engine, the renderer's graphics state over its backend's `Gpu`
//! (`context`) keeps the stores of what draws share, and the queue that
//! releases it in order.

pub(crate) mod atlas_binding;
pub(crate) mod buffer;
pub(crate) mod context;
pub(crate) mod draws;
pub(crate) mod dynamic_texture;
pub(crate) mod dynamic_texture_atlas;
pub(crate) mod identity;
pub(crate) mod indexes;
pub(crate) mod mesh;
pub(crate) mod objects;
pub(crate) mod owned;
pub(crate) mod release;
pub(crate) mod resources;
pub(crate) mod segment;
pub(crate) mod store;
pub(crate) mod target;
pub(crate) mod texture2d;
pub(crate) mod tile_render_data;
pub(crate) mod uniform_buffer;
pub(crate) mod upload;
pub(crate) mod vertex_attribute;
pub(crate) mod vertices;

/// The size of a texture of an image of `size`.
pub(crate) fn texture_size(size: mln_style::size::Size) -> mln_gfx::Size {
    mln_gfx::Size::new(size.width, size.height)
}

pub use crate::draw::draws::{Depth, Draw, Draws, Group, LayerFrame, LendAt, Lent};
pub use crate::draw::objects::Mesh;
pub use crate::draw::resources::Resources;
pub use crate::draw::target::Target;

/// What a program of a layer that stands on terrain or hides behind it
/// declares. A program of a layer whose type stands on terrain
/// ([`OnTerrain::Stands`](crate::layer::OnTerrain::Stands)) declares the
/// attribute [`GROUND_HEIGHT`](crate::draw::ground::GROUND_HEIGHT), which the engine fills, for a draw of its
/// terrain variant, with the height of the ground under each of the draw's
/// points; one whose type hides behind the ground in front of it declares the
/// texture [`GROUND_DEPTH`](crate::draw::ground::GROUND_DEPTH), which the engine binds to the depth of the
/// frame's ground.
pub mod ground {
    pub use crate::render::terrain::programs::{GROUND_DEPTH, GROUND_HEIGHT};
}
