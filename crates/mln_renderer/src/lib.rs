//! The map renderer: sources and tiles, layouts and buckets, render layers,
//! and the orchestrator that brings them to each frame's update.

mod algorithm;
#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench;
pub mod bucket;
mod bucket_geometry;
mod bucket_parameters;
mod buckets;
mod clip_masks;
#[cfg(feature = "gpu-conformance")]
pub mod conformance;
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
// Each layer type's programs and the engine's, public for the backends'
// tests, which compile them. Not API.
#[doc(hidden)]
pub mod layers;
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
// Each layer type's programs and the engine's, public for the backends'
// tests, which compile them. Not API.
#[doc(hidden)]
pub mod shaders;
mod sky_pass;
pub mod source_state;
mod source_type;
mod sources;
mod terrain;
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
pub use layers::custom::{
    Custom, CustomHost, CustomLayerFrame, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerRenderParameters,
};
pub(crate) use paint_property_binder::paint_property_binders;
pub use registry::{Registry, RegistryBuilder, register_builtins};
pub use renderer::{DrawnFrame, RenderTimings, Renderer, UpdatedFrame};
pub use renderer_event::{
    FontStack, GlyphRange, OverscaledTileID, RenderMode, RendererEvent, RendererEvents, RenderingStats, TileOperation,
};
pub use update_parameters::UpdateParameters;
pub use update_slot::{Update, UpdateReader, UpdateSlot};

/// The `log` targets of this crate's records. Each names the event that
/// `mln_c` reports the record under (`mln_c::logging`).
mod log_target {
    pub(crate) const GENERAL: &str = "mln_renderer::general";
    pub(crate) const RENDER: &str = "mln_renderer::render";
    pub(crate) const STYLE: &str = "mln_renderer::style";
    pub(crate) const GLYPH: &str = "mln_renderer::glyph";
    pub(crate) const TIMING: &str = "mln_renderer::timing";
}

#[cfg(test)]
mod tests;
