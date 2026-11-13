//! Ports `src/mln/renderer` (+ `include/mln/renderer`) and `util/tile_cover`.

pub mod algorithm;
pub mod bucket;
pub mod bucket_geometry;
pub mod bucket_parameters;
pub mod buckets;
pub mod change_request;
pub mod frame_projection;
pub mod geometry;
pub mod gfx;
pub mod globe_depth_pass;
pub mod globe_tile_mesh;
pub mod group_by_layout;
pub mod image_manager;
pub mod layer_group;
pub mod layer_tweaker;
pub mod layermanager;
pub mod layers;
pub mod layout;
pub mod line_atlas;
pub mod paint_parameters;
pub mod paint_property_binder;
pub mod paint_property_statistics;
pub mod pattern_atlas;
pub mod query;
pub mod render_layer;
pub mod render_light;
pub mod render_orchestrator;
pub mod render_pass;
pub mod render_sky;
pub mod render_source;
pub mod render_static_data;
pub mod render_target;
pub mod render_tile;
pub mod render_tree;
pub mod renderer;
pub mod renderer_event;
pub mod sky_pass;
pub mod source_state;
pub mod sources;
pub mod style_diff;
pub mod text;
pub mod tile;
pub mod tile_mask;
pub mod tile_parameters;
pub mod tile_pyramid;
pub mod tile_render_data;
pub mod update_parameters;
pub mod update_slot;
pub mod util;

pub use frame_projection::{ClipDepthRange, FrameProjection};
pub use renderer::Renderer;
pub use renderer_event::{
    FeatureInfo, FontStack, FrameRenderedFeaturesMap, GlyphRange, LayerFeaturesMap, NDCBound, OverscaledTileID,
    RenderMode, RendererEvent, RendererEvents, RenderingStats, SourceLayerID, TileOperation,
};
pub use update_parameters::UpdateParameters;
pub use update_slot::{Update, UpdateReader, UpdateSlot};
