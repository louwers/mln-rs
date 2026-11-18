//! The map renderer: sources and tiles, layouts and buckets, render layers,
//! and the orchestrator that turns a style into each frame's render tree.

mod algorithm;
#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench;
pub mod bucket;
mod bucket_geometry;
mod bucket_parameters;
mod buckets;
mod clip_masks;
mod draw;
mod frame;
mod frame_projection;
mod geometry;
mod gfx;
mod globe_depth_pass;
mod globe_tile_mesh;
mod group_by_layout;
mod image_store;
mod layer_composite;
mod layer_drawing;
mod layer_type;
mod layers;
mod layout;
mod line_atlas;
mod paint_parameters;
mod paint_property_binder;
mod paint_property_statistics;
mod pattern_atlas;
pub mod query;
mod registry;
mod render_layer;
mod render_light;
mod render_orchestrator;
mod render_pass;
mod render_sky;
mod render_source;
mod render_static_data;
mod render_target;
mod render_tile;
mod renderer;
mod renderer_event;
mod sky_pass;
pub mod source_state;
mod source_type;
mod sources;
pub mod text;
pub mod tile;
mod tile_mask;
mod tile_matrix;
mod tile_parameters;
mod tile_render_data;
mod update_parameters;
pub mod update_slot;
pub mod util;

pub use frame_projection::{ClipDepthRange, FrameProjection};
pub use gfx::context::Stats;
pub(crate) use paint_property_binder::paint_property_binders;
pub use registry::{Registry, RegistryBuilder, register_builtins};
pub use renderer::{RenderTimings, Renderer};
pub use renderer_event::{
    FontStack, GlyphRange, OverscaledTileID, RenderMode, RendererEvent, RendererEvents, RenderingStats, TileOperation,
};
pub use update_parameters::UpdateParameters;
pub use update_slot::{Update, UpdateReader, UpdateSlot};

#[cfg(test)]
mod tests;
