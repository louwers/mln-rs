// Port-only: layers, their evaluated properties and their expressions as
// serde values, for a tile worker in another wasm instance (mln_renderer's
// tile/worker_codec).
//
// A `Layer::Impl` and a `LayerProperties` are trait objects, so each crosses
// as the concrete type of its layer type, tagged with it. An expression
// crosses as its parsed node tree, so the other side doesn't parse it again;
// tools/render-test's `layer_round_trip` test sends every layer of the
// render and query suites through serde.
//
// Layers, layer properties and expressions cross once per pair of sides,
// then by key (mln_util::serialization::shared): every tile's `setLayers`
// has the same layers, and the layouts a worker sends back refer to the
// properties they were laid out with, and their binders to the layers'
// expressions.

use std::sync::Arc;

use mln_util::serialization::shared::{self, Kind};
use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::expression::Expr;
use crate::layer_impl::Impl;
use crate::layer_properties::LayerProperties;
use crate::layers::background_layer_properties::BackgroundLayerProperties;
use crate::layers::circle_layer_properties::CircleLayerProperties;
use crate::layers::color_relief_layer_properties::ColorReliefLayerProperties;
use crate::layers::fill_extrusion_layer_properties::FillExtrusionLayerProperties;
use crate::layers::fill_layer_properties::FillLayerProperties;
use crate::layers::heatmap_layer_properties::HeatmapLayerProperties;
use crate::layers::hillshade_layer_properties::HillshadeLayerProperties;
use crate::layers::line_layer_properties::LineLayerProperties;
use crate::layers::raster_layer_properties::RasterLayerProperties;
use crate::layers::symbol_layer_properties::SymbolLayerProperties;
use crate::layers::{
    background_layer_impl, circle_layer_impl, color_relief_layer_impl, fill_extrusion_layer_impl, fill_layer_impl,
    heatmap_layer_impl, hillshade_layer_impl, line_layer_impl, raster_layer_impl, symbol_layer_impl,
};

/// For each layer type: the tag, its `Layer::Impl` and its
/// `LayerProperties`.
macro_rules! layer_types {
    ($($tag:ident: $impl_:ty, $properties:ty;)+) => {
        #[derive(Serialize)]
        enum ImplRef<'a> { $($tag(&'a $impl_),)+ }

        #[expect(clippy::large_enum_variant, reason = "the value is read, then moved into an `Arc` at once")]
        #[derive(Deserialize)]
        enum ImplOwned { $($tag($impl_),)+ }

        #[derive(Serialize)]
        enum PropertiesRef<'a> { $($tag(&'a $properties),)+ }

        #[derive(Deserialize)]
        enum PropertiesOwned { $($tag($properties),)+ }

        fn impl_ref(impl_: &dyn Impl) -> Option<ImplRef<'_>> {
            $(if let Some(impl_) = impl_.downcast_ref::<$impl_>() {
                return Some(ImplRef::$tag(impl_));
            })+
            None
        }

        fn impl_arc(impl_: ImplOwned) -> Arc<dyn Impl> {
            match impl_ { $(ImplOwned::$tag(impl_) => Arc::new(impl_),)+ }
        }

        fn properties_ref(properties: &dyn LayerProperties) -> Option<PropertiesRef<'_>> {
            $(if let Some(properties) = properties.downcast_ref::<$properties>() {
                return Some(PropertiesRef::$tag(properties));
            })+
            None
        }

        fn properties_arc(properties: PropertiesOwned) -> Arc<dyn LayerProperties> {
            match properties { $(PropertiesOwned::$tag(properties) => Arc::new(properties),)+ }
        }
    };
}

layer_types! {
    Background: background_layer_impl::Impl, BackgroundLayerProperties;
    Circle: circle_layer_impl::Impl, CircleLayerProperties;
    ColorRelief: color_relief_layer_impl::Impl, ColorReliefLayerProperties;
    Fill: fill_layer_impl::Impl, FillLayerProperties;
    FillExtrusion: fill_extrusion_layer_impl::Impl, FillExtrusionLayerProperties;
    Heatmap: heatmap_layer_impl::Impl, HeatmapLayerProperties;
    Hillshade: hillshade_layer_impl::Impl, HillshadeLayerProperties;
    Line: line_layer_impl::Impl, LineLayerProperties;
    Raster: raster_layer_impl::Impl, RasterLayerProperties;
    Symbol: symbol_layer_impl::Impl, SymbolLayerProperties;
}

/// The `shared::Kind` of `Layer::Impl`s.
pub struct SharedLayer;

impl Kind for SharedLayer {
    type Value = dyn Impl;
    const NAME: &'static str = "layer";

    fn write<S: Serializer>(impl_: &dyn Impl, serializer: S) -> Result<S::Ok, S::Error> {
        let type_ = impl_.get_type_info().type_;
        impl_ref(impl_)
            .ok_or_else(|| S::Error::custom(format!("no serde form for {type_} layers")))?
            .serialize(serializer)
    }

    fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<dyn Impl>, D::Error> {
        ImplOwned::deserialize(deserializer).map(impl_arc)
    }
}

/// The `shared::Kind` of `LayerProperties`.
pub struct SharedLayerProperties;

impl Kind for SharedLayerProperties {
    type Value = dyn LayerProperties;
    const NAME: &'static str = "layer properties";

    fn write<S: Serializer>(properties: &dyn LayerProperties, serializer: S) -> Result<S::Ok, S::Error> {
        let type_ = properties.base().base_impl.get_type_info().type_;
        properties_ref(properties)
            .ok_or_else(|| S::Error::custom(format!("no serde form for {type_} layer properties")))?
            .serialize(serializer)
    }

    fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<dyn LayerProperties>, D::Error> {
        let properties = properties_arc(PropertiesOwned::deserialize(deserializer)?);
        // The impl crossed separately (`base_impl`); a mismatch is malformed.
        if properties_ref(&*properties).is_none() {
            return Err(D::Error::custom("layer properties of another layer type than their layer"));
        }
        Ok(properties)
    }
}

/// `#[serde(with)]` for an `Arc<dyn layer::Impl>`.
pub mod shared_layer {
    use super::{Arc, Deserializer, Impl, Serializer, SharedLayer, shared};

    pub fn serialize<S: Serializer>(impl_: &Arc<dyn Impl>, serializer: S) -> Result<S::Ok, S::Error> {
        shared::serialize::<SharedLayer, S>(impl_, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<dyn Impl>, D::Error> {
        shared::deserialize::<SharedLayer, D>(deserializer)
    }
}

/// `#[serde(with)]` for an `Arc<dyn LayerProperties>`.
pub mod shared_layer_properties {
    use super::{Arc, Deserializer, LayerProperties, Serializer, SharedLayerProperties, shared};

    pub fn serialize<S: Serializer>(properties: &Arc<dyn LayerProperties>, serializer: S) -> Result<S::Ok, S::Error> {
        shared::serialize::<SharedLayerProperties, S>(properties, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<dyn LayerProperties>, D::Error> {
        shared::deserialize::<SharedLayerProperties, D>(deserializer)
    }
}

/// The `shared::Kind` of the expressions behind `Arc`s.
pub struct SharedExpression;

impl Kind for SharedExpression {
    type Value = Expr;
    const NAME: &'static str = "expression";

    fn write<S: Serializer>(expression: &Expr, serializer: S) -> Result<S::Ok, S::Error> {
        Serialize::serialize(expression, serializer)
    }

    fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<Expr>, D::Error> {
        Expr::deserialize(deserializer).map(Arc::new)
    }
}

/// `#[serde(with)]` for an `Arc<Expr>`.
pub(crate) mod shared_expression {
    use super::{Arc, Deserializer, Expr, Serializer, SharedExpression, shared};

    pub(crate) fn serialize<S: Serializer>(expression: &Arc<Expr>, serializer: S) -> Result<S::Ok, S::Error> {
        shared::serialize::<SharedExpression, S>(expression, serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<Expr>, D::Error> {
        shared::deserialize::<SharedExpression, D>(deserializer)
    }

    /// `#[serde(with)]` for an `Option<Arc<Expr>>`.
    pub(crate) mod option {
        use super::{Arc, Deserializer, Expr, Serializer, SharedExpression, shared};

        #[expect(clippy::ref_option, reason = "`#[serde(with)]` passes the field by reference")]
        pub(crate) fn serialize<S: Serializer>(
            expression: &Option<Arc<Expr>>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            shared::serialize_option::<SharedExpression, S>(expression, serializer)
        }

        pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Arc<Expr>>, D::Error> {
            shared::deserialize_option::<SharedExpression, D>(deserializer)
        }
    }
}
