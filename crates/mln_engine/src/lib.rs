//! The map engine: the renderer and its frames (`render`), sources
//! (`source`), tiles and their workers (`tile_store`, `worker`), layers'
//! layouts and buckets (`layer`), placement, queries, and what draws them
//! (`draw`).

#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench;
mod buckets;
pub mod camera;
#[cfg(feature = "gpu-conformance")]
pub mod conformance;
mod draw;
mod geometry;
mod gfx;
pub mod image;
pub mod layer;
// Each layer type's programs and the engine's, public for the backends'
// tests, which compile them. Not API.
#[doc(hidden)]
pub mod layers;
mod layout;
mod line_atlas;
pub mod map;
#[cfg(feature = "sqlite")]
pub mod offline;
mod pattern_atlas;
pub mod placement;
pub mod query;
mod registry;
pub mod render;
// Each layer type's programs and the engine's, public for the backends'
// tests, which compile them. Not API.
#[doc(hidden)]
pub mod shaders;
pub mod source;
pub mod storage;
pub mod text;
pub mod tile;
pub mod tile_store;
mod util;
pub mod worker;

pub(crate) use layer::paint_property_binder::paint_property_binders;
pub use layers::custom::{
    Custom, CustomHost, CustomLayerFrame, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerRenderParameters,
};
pub use registry::{Registry, RegistryBuilder, register_builtins};
pub use render::events::{
    FontStack, GlyphRange, RenderMode, RendererEvent, RendererEvents, RenderingStats, TileOperation,
};
pub use render::renderer::{DrawnFrame, RenderTimings, Renderer, UpdatedFrame};
pub use render::update_parameters::UpdateParameters;
pub use render::update_slot::{Update, UpdateReader, UpdateSlot};

/// The `log` targets of this crate's records. Each names the event that
/// `mln_c` reports the record under (`mln_c::logging`).
mod log_target {
    pub(crate) const GENERAL: &str = "mln_engine::general";
    #[cfg_attr(not(feature = "sqlite"), expect(dead_code, reason = "only the database logs"))]
    pub(crate) const DATABASE: &str = "mln_engine::database";
    pub(crate) const HTTP_REQUEST: &str = "mln_engine::http_request";
    pub(crate) const PARSE_TILE: &str = "mln_engine::parse_tile";
    pub(crate) const SPRITE: &str = "mln_engine::sprite";
    pub(crate) const SETUP: &str = "mln_engine::setup";
    pub(crate) const PARSE_STYLE: &str = "mln_engine::parse_style";
    pub(crate) const RENDER: &str = "mln_engine::render";
    pub(crate) const STYLE: &str = "mln_engine::style";
    pub(crate) const GLYPH: &str = "mln_engine::glyph";
    pub(crate) const TIMING: &str = "mln_engine::timing";
}

#[cfg(test)]
mod tests;
