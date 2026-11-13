//! The bucket types: the vertex and index data a tile's layers draw.

pub mod circle_bucket;
pub mod debug_bucket; // layers: debug
pub mod fill_bucket;
pub mod fill_extrusion_bucket;
pub mod heatmap_bucket; // dem-heatmap
pub mod hillshade_bucket; // dem-heatmap
pub mod line_bucket;
pub mod raster_bucket;
pub mod symbol_bucket;

pub use circle_bucket::CircleBucket;
pub use fill_bucket::FillBucket;
pub use fill_extrusion_bucket::FillExtrusionBucket;
pub use line_bucket::LineBucket;
pub use raster_bucket::RasterBucket;
