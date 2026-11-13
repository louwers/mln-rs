use mln_conversion::Convertible;

use super::layer_factory::{LayerFactory, get_source};
use crate::layer::{Layer, LayerTypeInfo};
use crate::layers::color_relief_layer::{ColorReliefLayer, Impl};

pub struct ColorReliefLayerFactory;

impl LayerFactory for ColorReliefLayerFactory {
    fn get_type_info(&self) -> &'static LayerTypeInfo {
        Impl::static_type_info()
    }

    fn create_layer(&self, id: &str, value: &Convertible<'_>) -> Option<Box<dyn Layer>> {
        let source = get_source(value)?;
        Some(Box::new(ColorReliefLayer::new(id, &source)))
    }
}
