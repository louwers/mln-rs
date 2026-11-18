//! The renderer's graphics state over its backend's `Gpu` (`context`), the
//! resources it makes there (`texture2d`, `attachment`, `owned`,
//! `uniform_buffer`, `upload`), and the `src/mln/gfx` files that build
//! bucket geometry, which include the renderer's bucket headers.

pub(crate) mod atlas_binding;
pub(crate) mod attachment;
pub(crate) mod context;
pub(crate) mod dynamic_texture;
pub(crate) mod dynamic_texture_atlas;
pub(crate) mod fill_generator;
pub(crate) mod fill_large_mesh_arrays;
pub(crate) mod index_vector;
pub(crate) mod owned;
pub(crate) mod texture2d;
pub(crate) mod uniform_buffer;
pub(crate) mod upload;
