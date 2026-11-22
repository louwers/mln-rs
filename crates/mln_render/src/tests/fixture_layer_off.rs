//! The tests' layer types of raster and raster-dem sources (`tint` and
//! `shade`), on the layer contract they had before `Layer`: switched off
//! until their inputs come back.

use std::sync::Arc;

use mln_style::PropertyEvaluationParameters;
use mln_style::expression::{EvaluationContext, Value};
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyInfo, PropertyTable, PropertyVisitor};
use mln_style::property_value::StyleValue as _;
use mln_style_types::image::{ImagePositions, ImageType};
use mln_text::glyph::GlyphID;
use mln_util::error::Error;
use mln_vec_map::VecMap;

use crate::layer::{
    Bucket, BucketBase, BuildParams, DemContents, DrawContext, EvaluateCx, EvaluatedLayer, Evaluation,
    FeatureIndexBuilder, FeatureStates, GeometryUpload, GeometryVec, LayerBucket, LayerType, LayerValues, Layout,
    LayoutParameters, NoBucket, NoLayout, NoTileView, RasterView, Resolved, SourceLayer, layers_by_id,
};
use crate::layer_drawing::TileDraws;
use crate::tile_values::{ChangedBuckets, TileMap};

mln_style::properties!(
    /// The paint properties of a `tint` layer.
    #[expect(non_snake_case, reason = "named as the layer types' property sets are")]
    pub(crate) mod TintPaintProperties: paint {
        tint_opacity: constant f32 = 1.0_f32,
    }
);

static TINT_PROPERTIES: PropertyTable = PropertyTable::new(&[PropertyInfo::paint("tint-opacity")], &[0]);

/// The `tint` layer type.
pub(crate) struct Tint;

impl StyleLayerType for Tint {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "tint", source: SourceKind::Raster, from_json: true };
    const PROPERTIES: &'static PropertyTable = &TINT_PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = TintPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        Some(match slot {
            0 => visitor.paint(|s| &s.paint.tint_opacity, |s| &mut s.paint.tint_opacity),
            _ => return None,
        })
    }
}

impl LayerType for Tint {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type View<'a> = RasterView<'a>;
    type Tile = ();
    type Drawing = TileDraws;
    type State = ();

    fn new_layer(_layer: &LayerValues<Tint>) -> ((), TileDraws) {
        Default::default()
    }

    fn evaluate((): &mut (), _cx: EvaluateCx<'_, Tint>) -> Evaluation {
        Evaluation::TRANSLUCENT
    }

    fn make_draws(
        _state: &mut (),
        _drawing: &mut TileDraws,
        _tile_values: &mut TileMap<()>,
        _gpu: &mut dyn mln_gfx::gpu::Gpu,
        _cx: &mut DrawContext<'_, '_, Tint>,
    ) {
    }
}

static NO_PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

/// The `shade` layer type.
pub(crate) struct Shade;

impl StyleLayerType for Shade {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "shade", source: SourceKind::RasterDem, from_json: true };
    const PROPERTIES: &'static PropertyTable = &NO_PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

impl LayerType for Shade {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type View<'a> = &'a DemContents;
    type Tile = ();
    type Drawing = TileDraws;
    type State = ();

    fn new_layer(_layer: &LayerValues<Shade>) -> ((), TileDraws) {
        Default::default()
    }

    fn evaluate((): &mut (), _cx: EvaluateCx<'_, Shade>) -> Evaluation {
        Evaluation::TRANSLUCENT
    }

    fn make_draws(
        _state: &mut (),
        _drawing: &mut TileDraws,
        _tile_values: &mut TileMap<()>,
        _gpu: &mut dyn mln_gfx::gpu::Gpu,
        _cx: &mut DrawContext<'_, '_, Shade>,
    ) {
    }
}
