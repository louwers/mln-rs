//! The layer factories, which make a layer of each built-in type from its
//! style JSON for the [`StyleRegistry`](crate::registry::StyleRegistry).
//! Render layers, buckets and layouts need `mln_renderer`, which sits above
//! `mln_style`, so the renderer makes those (dispatch on `LayerTypeInfo`).

pub(crate) mod background_layer_factory;
pub(crate) mod circle_layer_factory;
pub(crate) mod color_relief_layer_factory;
pub(crate) mod fill_extrusion_layer_factory;
pub(crate) mod fill_layer_factory;
pub(crate) mod heatmap_layer_factory;
pub(crate) mod hillshade_layer_factory;
pub(crate) mod layer_factory;
pub(crate) mod line_layer_factory;
pub(crate) mod raster_layer_factory;
pub(crate) mod symbol_layer_factory;
