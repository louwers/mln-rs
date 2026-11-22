//! A Rust port of the MapLibre Native map rendering core.
//!
//! The root holds the thread-safe handles of `mln_runtime`: a [`Runtime`]
//! and its [`Map`]s, whose commands run on the runtime's thread and report
//! through a [`Completion`], and the [`RenderSession`]s that draw them, with
//! the traits through which they take the built-in source types' commands,
//! such as [`GeoJsonSourceCommands`]. It also assembles the built-in
//! expression functions, layer types and source types into the registry
//! that [`start_runtime`] starts a runtime with.
//!
//! Each extension point is one module, whose doc states its contract:
//! layer types ([`mln_render::layer`], drawing with [`mln_render::draw`]
//! and, on the globe, [`mln_render::globe`], whose author's names [`layer`]
//! gathers), source types and their tiles'
//! data origins ([`mln_render::source`]), expression functions
//! ([`mln_style::expression::functions`]), the programs a layer type draws
//! with, which carry their shader sources ([`mln_gfx::shaders::program`]),
//! and GPU backends ([`mln_gfx::gpu`]). The built-in layer and source types, in
//! [`mln_layers`] and [`mln_sources`], go through the same contracts.
//! [`register_builtins`] adds them to a [`RegistryBuilder`](mln_render::RegistryBuilder).
//! The first-party extensions are crates of their own, written against this
//! facade alone, which a caller adds to its registry as it would a third
//! party's: `mln_ngon`, `mln_location_puck` and `mln_image_process`.
//!
//! The library crates of <https://github.com/louwers/mln-rs> are re-exported
//! here under their crate names: `mln_rs::mln_style`, `mln_rs::mln_util`,
//! `mln_rs::mln_geojson`, and so on, with the worker codec that a layer
//! type's bucket implements, [`mln_wire`], and its `#[derive(Wire)]`. They
//! are published as the `mln-rs-*` packages, which only exist for this
//! crate.
//!
//! GeoJSON is the `geojson` crate's and feature geometry is `geo_types`'.
//! Both crates are re-exported.
//!
//! Each backend is a default feature (`webgpu`, `metal`, `vulkan`, `d3d12`,
//! `gl`) and so are SQLite (`sqlite`: the ambient cache, offline regions
//! and `mbtiles://` sources) and the in-process font-face shaper
//! (`font-face-shaper`). A browser build turns on one backend and leaves
//! the others out. A native runtime always has SQLite, so there `sqlite`
//! only re-exports `mln_storage_offline`.

mod builtins;
pub mod layer;

#[cfg(feature = "runtime")]
pub use builtins::start_runtime;
pub use builtins::{builtin_registry, register_builtins};
pub use feature;
pub use geo_types;
pub use geojson;
pub use geometry;
pub use glam;
/// Empty except on Windows.
#[cfg(feature = "d3d12")]
pub use mln_d3d12;
pub use mln_frame_pacer;
pub use mln_geojson;
pub use mln_gestures;
pub use mln_gfx;
#[cfg(feature = "gl")]
pub use mln_gl;
pub use mln_image;
pub use mln_layers;
pub use mln_map_types;
pub use mln_math;
/// Empty except on Apple platforms.
#[cfg(feature = "metal")]
pub use mln_mtl;
pub use mln_render;
// Every runtime type is at the root, so the root can't miss one that a
// root type's methods take or return.
#[cfg(feature = "runtime")]
pub use mln_runtime::{self, *};
pub use mln_sources;
// The built-in source types' commands and queries, which the runtime's maps
// and render sessions implement, with the values they take.
#[cfg(feature = "runtime")]
pub use mln_sources::{
    ClusterQueries, CustomSourceCallbacks, CustomSourceCommands, GeoJsonSourceCommands, GeoJsonSourceSettings,
    ImageSourceCommands,
};
pub use mln_storage;
#[cfg(feature = "sqlite")]
pub use mln_storage_offline;
pub use mln_style;
pub use mln_style_types;
pub use mln_system_fonts;
pub use mln_text;
pub use mln_tile;
pub use mln_tile_id;
pub use mln_util;
pub use mln_vec_map;
/// Empty on Apple platforms and in the browser build.
#[cfg(feature = "vulkan")]
pub use mln_vulkan;
/// Empty except in the browser build.
pub use mln_web_host;
#[cfg(feature = "webgpu")]
pub use mln_webgpu;
/// A crate that derives `Wire` through this re-export names it with
/// `#[wire(crate = ...)]`:
///
/// ```
/// #[derive(mln_rs::mln_wire::Wire)]
/// #[wire(crate = mln_rs::mln_wire)]
/// struct Dots {
///     positions: Vec<[i16; 2]>,
/// }
/// ```
pub use mln_wire;
pub use polylabel;
