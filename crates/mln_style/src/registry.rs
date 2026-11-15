//! The registrations that a style parses against: its expression functions
//! and its layer types.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use mln_conversion::{Convertible, Error};
use mln_vec_map::VecMap;

use crate::expression::Functions;
use crate::layer::Layer;
use crate::layermanager::background_layer_factory::BackgroundLayerFactory;
use crate::layermanager::circle_layer_factory::CircleLayerFactory;
use crate::layermanager::color_relief_layer_factory::ColorReliefLayerFactory;
use crate::layermanager::fill_extrusion_layer_factory::FillExtrusionLayerFactory;
use crate::layermanager::fill_layer_factory::FillLayerFactory;
use crate::layermanager::heatmap_layer_factory::HeatmapLayerFactory;
use crate::layermanager::hillshade_layer_factory::HillshadeLayerFactory;
use crate::layermanager::layer_factory::LayerFactory;
use crate::layermanager::line_layer_factory::LineLayerFactory;
use crate::layermanager::raster_layer_factory::RasterLayerFactory;
use crate::layermanager::symbol_layer_factory::SymbolLayerFactory;

/// The expression functions and layer types that a style's JSON, its layers'
/// property values and its filters parse against.
pub struct StyleRegistry {
    functions: Arc<Functions>,
    factories: VecMap<&'static str, Box<dyn LayerFactory>>,
}

impl StyleRegistry {
    /// The built-in layer types, with `functions`.
    #[must_use]
    pub fn new(functions: Arc<Functions>) -> Self {
        let factories: [Box<dyn LayerFactory>; 10] = [
            Box::new(FillLayerFactory),
            Box::new(LineLayerFactory),
            Box::new(CircleLayerFactory),
            Box::new(SymbolLayerFactory),
            Box::new(RasterLayerFactory),
            Box::new(BackgroundLayerFactory),
            Box::new(HillshadeLayerFactory),
            Box::new(ColorReliefLayerFactory),
            Box::new(FillExtrusionLayerFactory),
            Box::new(HeatmapLayerFactory),
        ];
        let factories = factories.into_iter().map(|factory| (factory.get_type_info().type_, factory)).collect();
        StyleRegistry { functions, factories }
    }

    /// The built-in layer types and expression functions.
    #[must_use]
    pub fn with_builtins() -> Arc<Self> {
        Arc::new(StyleRegistry::new(Functions::with_builtins()))
    }

    /// The expression functions.
    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        &self.functions
    }

    /// Feeds `hasher` what parsing and decoding resolve against: the
    /// functions' [`digest`](Functions::digest), then the layer types'
    /// names, in the order they were added.
    pub fn digest(&self, hasher: &mut impl Hasher) {
        self.functions.digest(hasher);
        self.factories.len().hash(hasher);
        for name in self.factories.keys() {
            name.hash(hasher);
        }
    }

    /// A layer of the type `type_` from its style JSON `value`, without its
    /// properties.
    pub(crate) fn create_layer(
        &self,
        type_: &str,
        id: &str,
        value: &Convertible<'_>,
        error: &mut Error,
    ) -> Option<Box<dyn Layer>> {
        let Some(factory) = self.factories.get(type_) else {
            error.message = format!("Unsupported layer type! Null factory for type: {type_}");
            return None;
        };
        let layer = factory.create_layer(id, value, &self.functions);
        if layer.is_none() {
            error.message = format!("Error parsing layer {id} of type: {type_}");
        }
        layer
    }
}

impl std::fmt::Debug for StyleRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleRegistry")
            .field("layer_types", &self.factories.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}
