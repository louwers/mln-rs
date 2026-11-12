//! Rendering without a window: `HeadlessFrontend`, ported from C++'s
//! `platform/default`.

pub mod headless_frontend;

pub use headless_frontend::{HeadlessFrontend, RenderResult};
