//! A Rust port of the MapLibre Native map rendering core.
//!
//! The root holds the thread-safe handles of `mln_runtime`: a [`Runtime`]
//! and its [`Map`]s, whose commands run on the runtime's thread and report
//! through a [`Completion`], and the [`RenderSession`]s that draw them. It
//! also assembles the built-in expression functions, layer types and source
//! types into the registry that [`start_runtime`] starts a runtime with.
//!
//! The library crates of <https://github.com/louwers/mln-rs> are re-exported
//! here under their crate names: `mln_rs::mln_map`, `mln_rs::mln_util`,
//! `mln_rs::mln_geojson`, and so on. They are published as the `mln-rs-*`
//! packages, which only exist for this crate. The tile worker codec's
//! encoding (`mln_wire`) is internal to the port and not re-exported.
//!
//! GeoJSON is the `geojson` crate's and feature geometry is `geo_types`'.
//! Both crates are re-exported.
//!
//! Each backend is a default feature (`webgpu`, `metal`, `vulkan`, `d3d12`,
//! `gl`) and so are SQLite (`sqlite`: the ambient cache, offline regions
//! and `mbtiles://` sources) and the in-process font-face shaper
//! (`font-face-shaper`). A browser build turns on one backend and leaves
//! the others out.

mod builtins;

pub use builtins::{builtin_registry, register_builtins, start_runtime};
pub use feature;
pub use geo_types;
pub use geojson;
pub use geometry;
pub use glam;
pub use mln_conversion;
/// Empty except on Windows.
#[cfg(feature = "d3d12")]
pub use mln_d3d12;
pub use mln_expression;
pub use mln_frame_pacer;
pub use mln_geojson;
pub use mln_gestures;
pub use mln_gfx;
#[cfg(feature = "gl")]
pub use mln_gl;
pub use mln_image;
pub use mln_map;
pub use mln_map_types;
pub use mln_math;
/// Empty except on Apple platforms.
#[cfg(feature = "metal")]
pub use mln_mtl;
pub use mln_renderer;
// Every runtime type is at the root, so the root can't miss one that a
// root type's methods take or return.
pub use mln_runtime::{self, *};
pub use mln_sprite;
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
pub use polylabel;
