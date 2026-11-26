//! A Rust port of the MapLibre Native map rendering core.
//!
//! The root holds the thread-safe handles of `mln_runtime`: a [`Runtime`]
//! and its [`Map`]s, whose commands run on the runtime's thread and report
//! through a [`Completion`], and the [`RenderSession`]s that draw them.
//!
//! The library crates of <https://github.com/maplibre/mln-rs> are re-exported
//! here under their crate names: `mln_rs::mln_engine`, `mln_rs::mln_util`,
//! `mln_rs::mln_geojson`, and so on. They are published as the `mln-rs-*`
//! packages, which only exist for this crate. The tile worker codec's
//! encoding (`mln_wire`) is internal to the port and not re-exported.
//!
//! The library logs through the `log` crate and installs no logger: install
//! a `log` backend to read its records. A record's target is
//! `<crate>::<event>`, where the event says what logged it, as the C API's
//! log events do: `mln_style::parse_style`, `mln_engine::database`,
//! `mln_vulkan::render`. The offline database's errors carry their SQLite or
//! I/O error code as a `code` key-value.
//!
//! GeoJSON is the `geojson` crate's and feature geometry is `geo_types`'.
//! Both crates are re-exported.
//!
//! Each backend is a feature, which re-exports its crate: `metal`
//! (`mln_mtl`), `vulkan` (`mln_vulkan`), `d3d12` (`mln_d3d12`), `gl`
//! (`mln_gl`) and `webgpu` (`mln_webgpu`). All but `webgpu` are on by
//! default; natively, wgpu renders through the same APIs as the others, so
//! turn it on to share a wgpu device or surface with the map.

pub use feature;
pub use geo_types;
pub use geojson;
pub use geometry;
pub use glam;
/// Empty except on Windows.
#[cfg(feature = "d3d12")]
pub use mln_d3d12;
pub use mln_engine;
pub use mln_frame_pacer;
pub use mln_geojson;
pub use mln_gestures;
pub use mln_gfx;
#[cfg(feature = "gl")]
pub use mln_gl;
pub use mln_image;
/// Empty except on Apple platforms.
#[cfg(feature = "metal")]
pub use mln_mtl;
// Every runtime type is at the root, so the root can't miss one that a
// root type's methods take or return.
pub use mln_runtime::{self, *};
pub use mln_style;
pub use mln_system_fonts;
pub use mln_text;
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
