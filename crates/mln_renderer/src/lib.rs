//! The map renderer: sources and tiles, layouts and buckets, render layers,
//! and the orchestrator that turns a style into each frame's render tree.

mod algorithm;
pub mod bucket;
mod bucket_geometry;
mod bucket_parameters;
mod buckets;
mod change_request;
mod frame_projection;
mod geometry;
mod gfx;
mod globe_depth_pass;
mod globe_tile_mesh;
mod group_by_layout;
mod image_manager;
mod layer_group;
mod layer_tweaker;
pub mod layermanager;
mod layers;
mod layout;
mod line_atlas;
mod paint_parameters;
mod paint_property_binder;
mod paint_property_statistics;
mod pattern_atlas;
pub mod query;
mod render_layer;
mod render_light;
mod render_orchestrator;
mod render_pass;
mod render_sky;
mod render_source;
mod render_static_data;
mod render_target;
mod render_tile;
mod render_tree;
mod renderer;
mod renderer_event;
mod sky_pass;
pub mod source_state;
mod sources;
mod style_diff;
pub mod text;
pub mod tile;
mod tile_mask;
mod tile_parameters;
mod tile_pyramid;
mod tile_render_data;
mod update_parameters;
pub mod update_slot;
pub mod util;

pub use frame_projection::{ClipDepthRange, FrameProjection};
pub(crate) use paint_property_binder::paint_property_binders;
pub use renderer::Renderer;
pub use renderer_event::{
    FeatureInfo, FontStack, FrameRenderedFeaturesMap, GlyphRange, LayerFeaturesMap, NDCBound, OverscaledTileID,
    RenderMode, RendererEvent, RendererEvents, RenderingStats, SourceLayerID, TileOperation,
};
pub use update_parameters::UpdateParameters;
pub use update_slot::{Update, UpdateReader, UpdateSlot};

#[cfg(test)]
mod tests;
