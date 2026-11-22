//! The map renderer: sources and tiles, layouts and buckets, render layers,
//! and the orchestrator that brings them to each frame's update.
//!
//! The renderer names no layer or source type of its own: it draws the
//! types of the [`Registry`] it is made with. Each module of an extension
//! point states its contract:
//!
//! - [`layer`]: a layer type's render half, with what it [`draw`]s with and
//!   the geometry that drawing on the [`globe`] needs.
//! - [`source`]: a source type's render half and the data origin of its
//!   tiles.
//! - [`RegistryBuilder`]: where both register, with the expression
//!   functions (`mln_style::expression::functions`).
//!
//! The GPU backend that a renderer draws through is `mln_gfx::gpu`'s
//! `Gpu`. A layer type draws with programs it declares
//! (`mln_gfx::shaders::program`), and the engine with its own.
//!
//! Not a contract yet, and hidden from the documentation: placement, in the
//! `placement` module. It can change or go in any release.

mod after_gpu;
mod algorithm;
#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench;
mod bucket;
mod bucket_geometry;
mod bucket_parameters;
mod bucket_upload;
mod buckets;
mod clip_masks;
pub mod draw;
mod draw_objects;
mod encode;
mod erased_layer;
mod erased_source;
mod frame;
mod frame_projection;
mod geometry;
mod gfx;
pub mod globe;
mod globe_depth_pass;
mod group_by_layout;
mod handles;
mod image_store;
pub mod layer;
mod layer_composite;
mod layout;
mod mesh_desc;
mod paint_parameters;
mod paint_property_binder;
mod paint_property_statistics;
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
mod render_tile;
mod renderer;
mod renderer_event;
mod shaders;
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
mod tile_values;
mod update_parameters;

pub use bucket::LayerFeatureStates;
pub use mln_style::expression::functions::RegisterError;
pub use registry::{Registry, RegistryBuilder};
pub use renderer::{DrawnFrame, Renderer, UpdatedFrame};
pub use renderer_event::{
    FontStack, GlyphRange, OverscaledTileID, RenderMode, RendererEvent, RendererEvents, RenderingStats, TileOperation,
};
pub use update_parameters::UpdateParameters;

/// What the expansion of [`paint_property_binders!`] names: the binders'
/// internals and the crates it reaches, so that a crate that invokes it
/// needn't depend on them.
#[doc(hidden)]
pub mod __private {
    pub use mln_style;
    pub use mln_style_types::image::ImagePositions;
    pub use mln_tile;
    pub use mln_tile_id;
    pub use mln_wire;

    pub use crate::bucket::FeatureStates;
    pub use crate::paint_property_binder::{
        Bind, BinderSet, InterleavedVertexBuffer, PaintPropertyBinderBase, PatternDependency, UniformValue,
    };
}

#[cfg(test)]
mod tests;
