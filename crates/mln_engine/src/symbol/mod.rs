//! The symbol layer type's render half: its layer (`layer`), the layout that
//! turns a tile's features into labels and icons (`layout`, `feature`,
//! `instance`), the anchors, lines and quads it places them along
//! (`get_anchors`, `anchor`, `check_max_angle`, `clip_lines`, `merge_lines`,
//! `quads`), its bucket (`bucket`), the projections that draw and place them
//! (`projection`), its part of placement, which decides which symbols show
//! (`placement`), and its programs, the blocks they read, and their text in
//! each language.

pub(crate) mod anchor;
pub(crate) mod bucket;
pub(crate) mod check_max_angle;
pub(crate) mod clip_lines;
pub(crate) mod feature;
pub(crate) mod get_anchors;
#[rustfmt::skip]
mod glsl;
#[rustfmt::skip]
mod glsl_es;
pub(crate) mod instance;
pub(crate) mod layer;
pub(crate) mod layout;
pub(crate) mod merge_lines;
#[rustfmt::skip]
mod msl;
pub(crate) mod placement;
pub mod programs;
pub(crate) mod projection;
pub(crate) mod quads;
pub mod ubo;
mod wgsl;
