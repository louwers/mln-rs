// Port-only: layers, their evaluated properties and their expressions in
// the `mln_wire` encoding, for a tile worker in another wasm instance
// (mln_renderer's tile/worker_codec).
//
// A `Layer::Impl` and a `LayerProperties` are trait objects, so each crosses
// as the concrete type of its layer type, tagged with it. An expression
// crosses as its parsed node tree, so the other side doesn't parse it again;
// tools/render-test's `layer_round_trip` test sends every layer of the
// render and query suites across.
//
// Layers, layer properties and expressions cross once per pair of sides,
// then by key (mln_util::serialization::shared): every tile's layout job
// has the same layers, and the layouts a worker sends back refer to the
// properties they were laid out with, and their binders to the layers'
// expressions.

use std::sync::Arc;

use mln_util::serialization::shared::{self, Kind, WireKind};
use mln_wire::{Reader, Wire, Writer};

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
/// `LayerProperties`. A layer or its properties cross as the index of its
/// type, then the concrete value.
macro_rules! layer_types {
    ($($tag:ident: $impl_:ty, $properties:ty;)+) => {
        fn write_impl(impl_: &dyn Impl, w: &mut Writer) {
            let mut index = 0u32;
            $(if let Some(impl_) = impl_.downcast_ref::<$impl_>() {
                mln_wire::write_tag(w, index);
                return impl_.write(w);
            }
            index += 1;)+
            let _ = index;
            w.fail(format!("no wire form for {} layers", impl_.get_type_info().type_));
        }

        fn read_impl(r: &mut Reader<'_>) -> Option<Arc<dyn Impl>> {
            let tag = r.tag()?;
            let mut index = 0u32;
            $(if tag == index {
                return Some(Arc::new(<$impl_>::read(r)?));
            }
            index += 1;)+
            let _ = index;
            r.fail_tag()
        }

        fn write_properties(properties: &dyn LayerProperties, w: &mut Writer) {
            let mut index = 0u32;
            $(if let Some(properties) = properties.downcast_ref::<$properties>() {
                mln_wire::write_tag(w, index);
                return properties.write(w);
            }
            index += 1;)+
            let _ = index;
            let type_ = properties.base().base_impl.get_type_info().type_;
            w.fail(format!("no wire form for {type_} layer properties"));
        }

        fn read_properties(r: &mut Reader<'_>) -> Option<Arc<dyn LayerProperties>> {
            let tag = r.tag()?;
            let mut index = 0u32;
            $(if tag == index {
                let properties = <$properties>::read(r)?;
                // The impl crossed separately (`base_impl`); a mismatch is
                // malformed.
                if properties.base.base_impl.downcast_ref::<$impl_>().is_none() {
                    return r.fail("layer properties of another layer type than their layer");
                }
                return Some(Arc::new(properties));
            }
            index += 1;)+
            let _ = index;
            r.fail_tag()
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
}

impl WireKind for SharedLayer {
    fn write(impl_: &dyn Impl, w: &mut Writer) {
        write_impl(impl_, w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<dyn Impl>> {
        read_impl(r)
    }
}

/// The `shared::Kind` of `LayerProperties`.
pub struct SharedLayerProperties;

impl Kind for SharedLayerProperties {
    type Value = dyn LayerProperties;
    const NAME: &'static str = "layer properties";
}

impl WireKind for SharedLayerProperties {
    fn write(properties: &dyn LayerProperties, w: &mut Writer) {
        write_properties(properties, w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<dyn LayerProperties>> {
        read_properties(r)
    }
}

/// The `shared::Kind` of the expressions behind `Arc`s.
pub struct SharedExpression;

impl Kind for SharedExpression {
    type Value = Expr;
    const NAME: &'static str = "expression";
}

impl WireKind for SharedExpression {
    fn write(expression: &Expr, w: &mut Writer) {
        expression.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<Expr>> {
        Expr::read(r).map(Arc::new)
    }
}

/// `#[wire(with)]` for an `Arc<dyn layer::Impl>`.
pub mod shared_layer {
    use super::{Arc, Impl, Reader, SharedLayer, Writer, shared};

    pub fn write(impl_: &Arc<dyn Impl>, w: &mut Writer) {
        shared::write::<SharedLayer>(impl_, w);
    }

    pub fn read(r: &mut Reader<'_>) -> Option<Arc<dyn Impl>> {
        shared::read::<SharedLayer>(r)
    }
}

/// `#[wire(with)]` for an `Arc<dyn LayerProperties>`.
pub mod shared_layer_properties {
    use super::{Arc, LayerProperties, Reader, SharedLayerProperties, Writer, shared};

    pub fn write(properties: &Arc<dyn LayerProperties>, w: &mut Writer) {
        shared::write::<SharedLayerProperties>(properties, w);
    }

    pub fn read(r: &mut Reader<'_>) -> Option<Arc<dyn LayerProperties>> {
        shared::read::<SharedLayerProperties>(r)
    }
}

/// `#[wire(with)]` for an `Arc<Expr>`.
pub(crate) mod shared_expression {
    use super::{Arc, Expr, Reader, SharedExpression, Writer, shared};

    pub(crate) fn write(expression: &Arc<Expr>, w: &mut Writer) {
        shared::write::<SharedExpression>(expression, w);
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Option<Arc<Expr>> {
        shared::read::<SharedExpression>(r)
    }

    /// `#[wire(with)]` for an `Option<Arc<Expr>>`.
    pub(crate) mod option {
        use super::{Arc, Expr, Reader, SharedExpression, Writer, shared};

        #[expect(clippy::ref_option, reason = "`#[wire(with)]` passes the field by reference")]
        pub(crate) fn write(expression: &Option<Arc<Expr>>, w: &mut Writer) {
            shared::write_option::<SharedExpression>(expression, w);
        }

        #[expect(clippy::option_option, reason = "a read that can fail, of an `Option`")]
        pub(crate) fn read(r: &mut Reader<'_>) -> Option<Option<Arc<Expr>>> {
            shared::read_option::<SharedExpression>(r)
        }
    }
}
