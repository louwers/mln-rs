//! The fill extrusions' programs, the blocks they read, and their text in each
//! language.

pub(crate) mod bucket;
pub(crate) mod layer;
#[rustfmt::skip]
mod glsl;
#[rustfmt::skip]
mod glsl_es;
#[rustfmt::skip]
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;

use mln_engine::draw::SegmentBase;

use crate::layers::fill_extrusion::bucket::FillExtrusionStaticVertex;

#[must_use]
pub(crate) fn fill_extrusion_vertices() -> Vec<FillExtrusionStaticVertex> {
    vec![[1, 0], [1, 1], [0, 0], [0, 1]]
}

#[must_use]
pub(crate) fn fill_extrusion_strip_indices() -> Vec<u16> {
    // ┌──────┐
    // │ 0  1 │ Counter-Clockwise winding order.
    // │      │ Triangle 1: 0 => 2 => 1
    // │ 2  3 │ Triangle 2: 1 => 2 => 3
    // └──────┘
    vec![0, 2, 1, 3]
}

/// A segment that draws `instance_count` fill-extrusion walls as a
/// triangle strip.
#[must_use]
pub(crate) fn fill_extrusion_segment(instance_count: usize) -> SegmentBase {
    SegmentBase { instance_count, ..SegmentBase::with_lengths(0, 0, 4, 4) }
}
