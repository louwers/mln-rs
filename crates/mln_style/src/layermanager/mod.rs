//! The style half of include/mln/layermanager and src/mln/layermanager
//! (`LayerFactory::createLayer`, `LayerManager::createLayer`). The C++
//! factories also create render layers, buckets and layouts; those parts
//! need `mln_renderer`, which sits above `mln_style`, so they stay with the
//! renderer (dispatch on `LayerTypeInfo`). A cut point of the C++ include
//! cycle.

pub(crate) mod background_layer_factory;
pub(crate) mod circle_layer_factory;
pub(crate) mod color_relief_layer_factory;
pub(crate) mod fill_extrusion_layer_factory;
pub(crate) mod fill_layer_factory;
pub(crate) mod heatmap_layer_factory;
pub(crate) mod hillshade_layer_factory;
pub(crate) mod layer_factory;
pub(crate) mod layer_manager;
pub(crate) mod line_layer_factory;
pub(crate) mod raster_layer_factory;
pub(crate) mod symbol_layer_factory;

pub(crate) use layer_manager::LayerManager;
