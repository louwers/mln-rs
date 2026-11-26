//! The render half of the layer types: how a tile lays out a layer's
//! features into buckets, what its paint properties bind, and what a layer
//! keeps and draws each frame.

pub mod bucket;
pub(crate) mod bucket_geometry;
pub(crate) mod bucket_parameters;
pub(crate) mod filter_groups;
pub(crate) mod group_by_layout;
pub(crate) mod layer_drawing;
pub(crate) mod layer_type;
pub(crate) mod layout;
pub(crate) mod paint_property_binder;
pub(crate) mod paint_property_statistics;
pub(crate) mod render_layer;
pub(crate) mod tile_layout;
