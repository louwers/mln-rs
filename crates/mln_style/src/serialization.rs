// Port-only: layers, their evaluated properties and their expressions as
// serde values, for a tile worker in another wasm instance (mln_renderer's
// tile/worker_codec).
//
// A `Layer::Impl` and a `LayerProperties` are trait objects, so each crosses
// as the concrete type of its layer type, tagged with it. An expression
// crosses as what the style specification defines it by, the JSON
// `Expr::serialize` writes, with its type, and is parsed back with that type
// as the expected one. Where C++'s serialization doesn't parse back (the legacy property
// functions' errors and steps, literal images, the image and
// variable-anchor-offset coercions, cubic-bezier control points, colors),
// the port writes the JSON that parses back to the same expression; each
// such `serialize` says how, and tools/render-test's `layer_round_trip` test
// sends every layer of the render and query suites through serde.
//
// The symbol layer's `FormatSectionOverride`, an expression that has no
// style JSON, crosses as its parts.
//
// Layers, layer properties and expressions cross once per pair of sides,
// then by key (mln_util::serialization::shared): every tile's `setLayers`
// has the same layers, and the layouts a worker sends back refer to the
// properties they were laid out with, and their binders to the layers'
// expressions.

use std::sync::Arc;

use feature::Value;
use mln_conversion::Convertible;
use mln_expression::format_section_override::{FormatSectionOverride, OverrideDefault};
use mln_util::Color;
use mln_util::serialization::shared::{self, Kind};
use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::expression::{Expr, GlobalStateMap, ParsingContext, Type};
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

/// What an expression crosses as.
#[derive(Serialize, Deserialize)]
enum ExpressionDefinition {
    /// Its style JSON and type.
    Json(Value, Type),
    FormatSectionOverride {
        default_value: OverrideDefaultParts,
        property_name: String,
    },
}

/// What a `format-section-override`'s default crosses as.
#[derive(Serialize, Deserialize)]
enum OverrideDefaultParts {
    Constant(Color),
    Expression {
        #[serde(with = "shared_expression")]
        expression: Arc<Expr>,
        captured_global_state: Option<Arc<GlobalStateMap>>,
    },
}

impl Kind for SharedExpression {
    type Value = Expr;
    const NAME: &'static str = "expression";

    fn write<S: Serializer>(expression: &Expr, serializer: S) -> Result<S::Ok, S::Error> {
        let definition = match expression.as_format_section_override() {
            Some(format_section_override) => ExpressionDefinition::FormatSectionOverride {
                default_value: match format_section_override.default_value() {
                    OverrideDefault::Constant(color) => OverrideDefaultParts::Constant(*color),
                    OverrideDefault::Expression { expression, captured_global_state } => {
                        OverrideDefaultParts::Expression {
                            expression: Arc::clone(expression),
                            captured_global_state: captured_global_state.clone(),
                        }
                    }
                },
                property_name: format_section_override.property_name().to_owned(),
            },
            None => ExpressionDefinition::Json(expression.serialize(), expression.get_type()),
        };
        definition.serialize(serializer)
    }

    fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<Expr>, D::Error> {
        Ok(match ExpressionDefinition::deserialize(deserializer)? {
            ExpressionDefinition::Json(json, type_) => Arc::new(parse(&json, type_).map_err(D::Error::custom)?),
            ExpressionDefinition::FormatSectionOverride { default_value, property_name } => {
                let default_value = match default_value {
                    OverrideDefaultParts::Constant(color) => OverrideDefault::Constant(color),
                    OverrideDefaultParts::Expression { expression, captured_global_state } => {
                        OverrideDefault::Expression { expression, captured_global_state }
                    }
                };
                Arc::new(FormatSectionOverride::new_expr(default_value, property_name))
            }
        })
    }
}

/// Parses an expression's serialized form, expecting `type_`.
pub(crate) fn parse(json: &Value, type_: Type) -> Result<Expr, String> {
    let mut context = ParsingContext::with_expected(type_);
    context.parse_expression(&Convertible::from(json), None).ok_or_else(|| context.get_combined_errors())
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
