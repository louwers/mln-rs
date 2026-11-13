//! The `src/mln/gfx` files that build bucket geometry. They include the
//! renderer's bucket headers (vertex types and layout functions), so they
//! live in `mln_renderer` rather than in `mln_gfx`.

pub(crate) mod drawable_atlases_tweaker;
pub(crate) mod dynamic_texture;
pub(crate) mod dynamic_texture_atlas;
pub(crate) mod fill_generator;
pub(crate) mod fill_large_mesh_arrays;
