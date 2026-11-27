//! The fills' programs, the blocks they read, and their text in each
//! language.

pub(crate) mod bucket;
pub(crate) mod layer;
pub(crate) mod mesh;
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

/// The corners of the quad that an instanced fill outline draws a
/// segment with: x picks the segment's start (0) or end (1), y its side.
#[must_use]
pub(crate) fn fill_outline_vertices() -> Vec<[i16; 2]> {
    vec![[0, 0], [0, 1], [1, 0], [1, 1]]
}

#[must_use]
pub(crate) fn quad_strip_indices() -> Vec<u16> {
    vec![0, 1, 2, 3]
}

/// A segment that draws `instance_count` fill outline segments, as
/// triangle-strip quads of `fill_outline_vertices` and `quad_strip_indices`.
#[must_use]
pub(crate) fn fill_outline_segment(instance_count: usize) -> SegmentBase {
    SegmentBase { instance_count, ..SegmentBase::with_lengths(0, 0, 4, 4) }
}
