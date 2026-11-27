//! The renderer and each frame: its update, its passes and targets, and what
//! the frame draws beyond the layers (clip masks, sky, light, the globe and
//! terrain).

pub(crate) mod clip_masks;
pub(crate) mod events;
pub(crate) mod frame;
pub(crate) mod frame_projection;
pub(crate) mod globe;
pub(crate) mod layer_composite;
pub(crate) mod light;
pub(crate) mod orchestrator;
pub(crate) mod paint_parameters;
pub(crate) mod pass;
pub(crate) mod renderer;
pub(crate) mod shared_targets;
pub(crate) mod sky;
pub(crate) mod sky_pass;
pub(crate) mod static_data;
pub(crate) mod terrain;
pub(crate) mod tile;
pub(crate) mod tile_matrix;
pub(crate) mod update_parameters;
pub(crate) mod update_slot;
