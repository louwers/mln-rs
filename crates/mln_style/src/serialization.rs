// Port-only: layers' style data and their expressions in the `mln_wire`
// encoding, for a tile worker in another wasm instance (mln_renderer's
// tile/worker_codec).
//
// A layer's style data crosses as its type's name, then the value, and the
// reading side resolves the name against the registry of its
// `registry::decode_with`. An expression crosses as its parsed node tree, so
// the other side doesn't parse it again; tools/render-test's
// `layer_round_trip` test sends every layer of the render and query suites
// across.
//
// Layers and expressions cross once per pair of sides, then by key
// (mln_util::serialization::shared): every tile's layout job has the same
// layers, and the buckets a worker sends back refer to the layers'
// expressions through their binders.

use std::sync::Arc;

use mln_util::serialization::shared::{self, Kind, WireKind};
use mln_wire::{Reader, Wire, Writer};

use crate::expression::Expr;
use crate::layer_type::ErasedLayerStyle;
use crate::registry::decoding_layer_type;

/// The `shared::Kind` of layers' style data.
pub struct SharedLayer;

impl Kind for SharedLayer {
    type Value = dyn ErasedLayerStyle;
    const NAME: &'static str = "layer";
}

impl WireKind for SharedLayer {
    fn write(impl_: &dyn ErasedLayerStyle, w: &mut Writer) {
        mln_wire::write_str(w, impl_.info().name);
        impl_.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<dyn ErasedLayerStyle>> {
        let name = mln_wire::read_str(r)?;
        match decoding_layer_type(name) {
            Ok(layer_type) => layer_type.read_style(r),
            Err(message) => r.fail(message),
        }
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

/// `#[wire(with)]` for an `Arc<dyn ErasedLayerStyle>`.
pub mod shared_layer {
    use super::{Arc, ErasedLayerStyle, Reader, SharedLayer, Writer, shared};

    pub fn write(impl_: &Arc<dyn ErasedLayerStyle>, w: &mut Writer) {
        shared::write::<SharedLayer>(impl_, w);
    }

    pub fn read(r: &mut Reader<'_>) -> Option<Arc<dyn ErasedLayerStyle>> {
        shared::read::<SharedLayer>(r)
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
