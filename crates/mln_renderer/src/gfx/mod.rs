//! The renderer's graphics state over its backend's `Gpu` (`context`): what
//! makes its buffers and textures (`resources`, `buffer`), the stores of
//! what its draws share (`store`, `texture2d`, `indexes`, `vertices`), the
//! queue that releases it in order (`release`), the attributes that read
//! vertices (`vertex_attribute`), the bindings and geometry made of them
//! (`owned`, `uniform_buffer`, `upload`), and the meshes of lines and fills
//! (`line_mesh`, `fill_mesh`) and the segments that draw them (`segment`,
//! `mesh`).

pub(crate) mod atlas_binding;
pub(crate) mod buffer;
pub(crate) mod context;
pub(crate) mod dynamic_texture;
pub(crate) mod dynamic_texture_atlas;
pub(crate) mod fill_mesh;
pub(crate) mod indexes;
pub(crate) mod line_mesh;
pub(crate) mod mesh;
pub(crate) mod owned;
pub(crate) mod release;
pub(crate) mod resources;
pub(crate) mod segment;
pub(crate) mod store;
pub(crate) mod texture2d;
pub(crate) mod uniform_buffer;
pub(crate) mod upload;
pub(crate) mod vertex_attribute;
pub(crate) mod vertices;

/// The size of a texture of an image of `size`.
pub(crate) fn texture_size(size: mln_style::size::Size) -> mln_gfx::Size {
    mln_gfx::Size::new(size.width, size.height)
}
