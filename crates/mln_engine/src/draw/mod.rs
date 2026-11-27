//! The renderer's graphics state over its backend's `Gpu` (`context`): what
//! makes its buffers and textures (`resources`, `buffer`), the stores of
//! what its draws share (`store`, `texture2d`, `indexes`, `vertices`), the
//! queue that releases it in order (`release`), the attributes that read
//! vertices (`vertex_attribute`), the bindings and geometry made of them
//! (`owned`, `uniform_buffer`, `upload`), the segments that draw meshes
//! (`segment`, `mesh`), the GPU objects that draws draw, with the buffers
//! that frames bind them with (`objects`), the layers' draws of a frame,
//! which the engine holds and draws (`draws`), and the textures of a
//! layer's own that its draws draw into ahead of the frame (`target`).

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
