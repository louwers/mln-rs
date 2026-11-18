//! The renderer's graphics state over its backend's `Gpu` (`context`), the
//! stores of what its draws share there (`store`, `texture2d`, `indexes`,
//! `vertices`), the attributes that read vertices (`vertex_attribute`),
//! the other resources it makes there (`attachment`, `owned`,
//! `uniform_buffer`, `upload`), and what builds bucket geometry.

pub(crate) mod atlas_binding;
pub(crate) mod attachment;
pub(crate) mod context;
pub(crate) mod dynamic_texture;
pub(crate) mod dynamic_texture_atlas;
pub(crate) mod fill_generator;
pub(crate) mod fill_large_mesh_arrays;
pub(crate) mod indexes;
pub(crate) mod owned;
pub(crate) mod store;
pub(crate) mod texture2d;
pub(crate) mod uniform_buffer;
pub(crate) mod upload;
pub(crate) mod vertex_attribute;
pub(crate) mod vertices;
