//! The renderer's graphics state over its backend's `Gpu` (`context`), the
//! resources it makes there (`texture2d`, `attachment`, `bind_group`,
//! `uniform_buffer`, `upload`), and the `src/mln/gfx` files that build
//! bucket geometry, which include the renderer's bucket headers.

pub(crate) mod attachment;
pub(crate) mod bind_group;
pub(crate) mod context;
pub(crate) mod drawable_atlases_tweaker;
pub(crate) mod dynamic_texture;
pub(crate) mod dynamic_texture_atlas;
pub(crate) mod fill_generator;
pub(crate) mod fill_large_mesh_arrays;
pub(crate) mod texture2d;
pub(crate) mod uniform_buffer;
pub(crate) mod upload;
