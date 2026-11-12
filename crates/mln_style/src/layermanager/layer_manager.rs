// Ports include/mln/layermanager/layer_manager.hpp,
// src/mln/layermanager/layer_manager.cpp and
// platform/default/src/mln/layermanager/layer_manager.cpp
// (`LayerManagerDefault`), style half.

use std::sync::OnceLock;

use mln_conversion::{Convertible, Error};
use mln_vec_map::VecMap;

use super::background_layer_factory::BackgroundLayerFactory;
use super::circle_layer_factory::CircleLayerFactory;
use super::color_relief_layer_factory::ColorReliefLayerFactory;
use super::fill_extrusion_layer_factory::FillExtrusionLayerFactory;
use super::fill_layer_factory::FillLayerFactory;
use super::heatmap_layer_factory::HeatmapLayerFactory;
use super::hillshade_layer_factory::HillshadeLayerFactory;
use super::layer_factory::LayerFactory;
use super::line_layer_factory::LineLayerFactory;
use super::location_indicator_layer_factory::LocationIndicatorLayerFactory;
use super::raster_layer_factory::RasterLayerFactory;
use super::symbol_layer_factory::SymbolLayerFactory;
use crate::layer::Layer;

pub struct LayerManager {
    factories: Vec<Box<dyn LayerFactory>>,
    type_to_factory: VecMap<String, usize>,
}

impl LayerManager {
    /// A singleton getter.
    pub fn get() -> &'static LayerManager {
        static INSTANCE: OnceLock<LayerManager> = OnceLock::new();
        INSTANCE.get_or_init(LayerManager::new_default)
    }

    /// `LayerManagerDefault::LayerManagerDefault()`.
    fn new_default() -> Self {
        let mut manager = LayerManager { factories: Vec::new(), type_to_factory: VecMap::new() };
        manager.add_layer_type(Box::new(FillLayerFactory));
        manager.add_layer_type(Box::new(LineLayerFactory));
        manager.add_layer_type(Box::new(CircleLayerFactory));
        manager.add_layer_type(Box::new(SymbolLayerFactory));
        manager.add_layer_type(Box::new(RasterLayerFactory));
        manager.add_layer_type(Box::new(BackgroundLayerFactory));
        manager.add_layer_type(Box::new(HillshadeLayerFactory));
        manager.add_layer_type(Box::new(ColorReliefLayerFactory));
        manager.add_layer_type(Box::new(FillExtrusionLayerFactory));
        manager.add_layer_type(Box::new(HeatmapLayerFactory));
        // The custom layer factory is registered only with the OpenGL
        // backend (MLN_RENDER_BACKEND_OPENGL); the port renders with WebGPU.
        manager.add_layer_type(Box::new(LocationIndicatorLayerFactory));
        manager
    }

    fn add_layer_type(&mut self, factory: Box<dyn LayerFactory>) {
        let type_ = factory.get_type_info().type_.to_owned();
        if self.type_to_factory.contains_key(&type_) {
            // C++ asserts.
            debug_assert!(false, "duplicate layer type {type_}");
            return;
        }
        self.type_to_factory.insert(type_, self.factories.len());
        self.factories.push(factory);
    }

    fn get_factory(&self, type_: &str) -> Option<&dyn LayerFactory> {
        self.type_to_factory.get(type_).map(|&i| self.factories[i].as_ref())
    }

    /// Returns a new Layer instance on success call; returns `None` otherwise.
    pub fn create_layer(
        &self,
        type_: &str,
        id: &str,
        value: &Convertible<'_>,
        error: &mut Error,
    ) -> Option<Box<dyn Layer>> {
        if let Some(factory) = self.get_factory(type_) {
            let layer = factory.create_layer(id, value);
            if layer.is_none() {
                error.message = format!("Error parsing layer {id} of type: {type_}");
            }
            return layer;
        }
        error.message = format!("Null factory for type: {type_}");
        error.message = format!("Unsupported layer type! {}", error.message);
        None
    }
}
