//! The style half of include/mln/layermanager and src/mln/layermanager
//! (`LayerFactory::createLayer`, `LayerManager::createLayer`). The C++
//! factories also create render layers, buckets and layouts; those parts
//! need mln_renderer, which sits above mln_style, so they stay with the
//! renderer (dispatch on `LayerTypeInfo`). A cut point; see PLAN.md, "Crate map".

pub mod background_layer_factory;
pub mod circle_layer_factory;
pub mod color_relief_layer_factory;
pub mod fill_extrusion_layer_factory;
pub mod fill_layer_factory;
pub mod heatmap_layer_factory;
pub mod hillshade_layer_factory;
pub mod layer_factory;
pub mod layer_manager;
pub mod line_layer_factory;
pub mod location_indicator_layer_factory;
pub mod raster_layer_factory;
pub mod symbol_layer_factory;

pub use layer_factory::LayerFactory;
pub use layer_manager::LayerManager;
