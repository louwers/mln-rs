//! The heatmaps' programs, the blocks they read, and their text in each
//! language.

pub(crate) mod bucket;
pub(crate) mod layer;
pub(crate) mod layout;
#[rustfmt::skip]
mod glsl;
#[rustfmt::skip]
mod glsl_es;
#[rustfmt::skip]
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;

#[must_use]
pub(crate) fn heatmap_texture_vertices() -> Vec<bucket::HeatmapTextureLayoutVertex> {
    use geometry::Point;

    use crate::layers::heatmap::bucket::HeatmapBucket;
    vec![
        HeatmapBucket::texture_vertex(Point::new(0, 0)),
        HeatmapBucket::texture_vertex(Point::new(1, 0)),
        HeatmapBucket::texture_vertex(Point::new(0, 1)),
        HeatmapBucket::texture_vertex(Point::new(1, 1)),
    ]
}
