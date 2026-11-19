//! The renderer's graphics state over its backend's `Gpu` (`context`): what
//! makes its buffers and textures (`resources`, `buffer`), the stores of
//! what its draws share (`store`, `texture2d`, `indexes`, `vertices`), the
//! queue that releases it in order (`release`), the attributes that read
//! vertices (`vertex_attribute`), the bindings and geometry made of them
//! (`owned`, `uniform_buffer`, `upload`), the programs it draws with
//! (`shader_registry`, `shader_group`, `shader_program`), and what builds
//! bucket geometry.

pub(crate) mod atlas_binding;
pub(crate) mod buffer;
pub(crate) mod context;
pub(crate) mod dynamic_texture;
pub(crate) mod dynamic_texture_atlas;
pub(crate) mod fill_generator;
pub(crate) mod fill_large_mesh_arrays;
pub(crate) mod indexes;
pub(crate) mod owned;
pub(crate) mod release;
pub(crate) mod resources;
pub(crate) mod shader_group;
pub(crate) mod shader_program;
pub(crate) mod shader_registry;
pub(crate) mod store;
pub(crate) mod texture2d;
pub(crate) mod uniform_buffer;
pub(crate) mod upload;
pub(crate) mod vertex_attribute;
pub(crate) mod vertices;
