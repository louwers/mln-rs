use std::sync::Arc;

use mln_conversion::Convertible;

use super::layer_factory::LayerFactory;
use crate::expression::Functions;
use crate::layer::{Layer, LayerTypeInfo};
use crate::layers::background_layer::{BackgroundLayer, Impl};

pub(crate) struct BackgroundLayerFactory;

impl LayerFactory for BackgroundLayerFactory {
    fn get_type_info(&self) -> &'static LayerTypeInfo {
        Impl::static_type_info()
    }

    fn create_layer(&self, id: &str, _value: &Convertible<'_>, functions: &Arc<Functions>) -> Option<Box<dyn Layer>> {
        Some(Box::new(BackgroundLayer::with_functions(Arc::clone(functions), id)))
    }
}
