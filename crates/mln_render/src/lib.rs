//! The map renderer: sources and tiles, layouts and buckets, render layers,
//! and the orchestrator that brings them to each frame's update.
//!
//! The renderer names no layer or source type of its own: it draws the
//! types of the [`Registry`] it is made with. Each module of an extension
//! point states its contract:
//!
//! - [`layer`]: a layer type's render half, with the [`draw`] services it
//!   draws with.
//! - [`source`]: a source type's render half and the data origin of its
//!   tiles.
//! - [`RegistryBuilder`]: where both register, with the expression
//!   functions (`mln_style::expression::functions`) and the layer types'
//!   shader sources (`mln_gfx::shaders::source`).
//!
//! The GPU backend that a renderer draws through is `mln_gfx::gpu`'s
//! `Gpu`. What the engine exposes to the built-in types besides, not yet
//! meant as a contract, is in the hidden `unstable` module.

mod algorithm;
#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench;
mod bucket;
mod bucket_geometry;
mod bucket_parameters;
mod buckets;
mod clip_masks;
pub mod draw;
mod draw_objects;
mod erased_layer;
mod erased_source;
mod frame;
mod frame_projection;
mod geometry;
mod gfx;
mod globe_depth_pass;
mod globe_tile_mesh;
mod group_by_layout;
mod image_store;
pub mod layer;
mod layer_composite;
mod layer_drawing;
mod layout;
mod line_atlas;
mod paint_parameters;
mod paint_property_binder;
mod paint_property_statistics;
mod pattern_atlas;
#[doc(hidden)]
pub mod placement;
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
pub mod source;
pub mod source_state;
mod sources;
#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub mod testing;
pub mod text;
pub mod tile;
mod tile_mask;
mod tile_matrix;
mod tile_parameters;
mod tile_render_data;
#[doc(hidden)]
pub mod unstable;
mod update_parameters;
pub mod update_slot;
mod util;

pub use bucket::LayerFeatureStates;
pub use frame_projection::{ClipDepthRange, FrameProjection};
pub use mln_style::expression::functions::RegisterError;
pub use registry::{Registry, RegistryBuilder};
pub use renderer::{DrawnFrame, RenderTimings, Renderer, UpdatedFrame};
pub use renderer_event::{
    FontStack, GlyphRange, OverscaledTileID, RenderMode, RendererEvent, RendererEvents, RenderingStats, TileOperation,
};
pub use update_parameters::UpdateParameters;
pub use update_slot::{Update, UpdateReader, UpdateSlot};

/// The crates that the expansion of [`paint_property_binders!`] names, so
/// that a crate that invokes it needn't depend on them.
#[doc(hidden)]
pub mod __private {
    pub use mln_style;
    pub use mln_style_types;
    pub use mln_tile;
    pub use mln_tile_id;
    pub use mln_wire;
}

#[cfg(test)]
mod tests;
