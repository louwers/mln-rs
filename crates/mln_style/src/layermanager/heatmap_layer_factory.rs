use mln_conversion::Convertible;

use super::layer_factory::{LayerFactory, get_source};
use crate::layer::{Layer, LayerTypeInfo};
use crate::layers::heatmap_layer::{HeatmapLayer, Impl};

pub(crate) struct HeatmapLayerFactory;

impl LayerFactory for HeatmapLayerFactory {
    fn get_type_info(&self) -> &'static LayerTypeInfo {
        Impl::static_type_info()
    }

    fn create_layer(&self, id: &str, value: &Convertible<'_>) -> Option<Box<dyn Layer>> {
        let source = get_source(value)?;
        Some(Box::new(HeatmapLayer::new(id, &source)))
    }
}
