//! The tests' layer types of raster, raster-dem and no source (`tint`,
//! `shade` and the placing `mark`), on the layer contract they had before
//! `Layer`: switched off until their inputs and placement's registration
//! come back.


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
use crate::placement::{CollisionGroups, Placed, PlacementContext, PlacementPass, PlacementQuery, Prepared};
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

/// The `mark` layer type, and with `OTHER` the `other-mark` one.
pub(crate) struct Mark<const OTHER: bool = false>;

impl<const OTHER: bool> StyleLayerType for Mark<OTHER> {
    const INFO: LayerTypeInfo =
        LayerTypeInfo { name: if OTHER { "other-mark" } else { "mark" }, source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &NO_PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

impl<const OTHER: bool> LayerType for Mark<OTHER> {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type View<'a> = NoTileView;
    type Tile = ();
    type Drawing = TileDraws;
    type State = ();

    const PLACEMENT: Option<crate::placement::NewPlacement<Self>> = Some(|| Box::new(MarkPass::default()));

    fn new_layer(_layer: &LayerValues<Self>) -> ((), TileDraws) {
        Default::default()
    }

    fn evaluate((): &mut (), _cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation::TRANSLUCENT
    }

    fn make_draws(
        _state: &mut (),
        _drawing: &mut TileDraws,
        _tile_values: &mut TileMap<()>,
        _gpu: &mut dyn mln_gfx::gpu::Gpu,
        _cx: &mut DrawContext<'_, '_, Self>,
    ) {
    }
}

/// The placement pass of `mark` layers: it places nothing, and records the
/// hooks the engine calls, by name, with the number of layers of those that
/// see them.
#[derive(Default)]
pub(crate) struct MarkPass {
    /// Whether [`prepare`](PlacementPass::prepare) says a layer has
    /// something to place.
    pub(crate) places: bool,
    pub(crate) calls: Vec<String>,
}

impl<const OTHER: bool> PlacementPass<Mark<OTHER>> for MarkPass {
    fn prepare(&mut self, cx: &mut PlacementContext<'_, Mark<OTHER>>) -> Prepared {
        self.calls.push(format!("prepare {}", cx.layers.len()));
        Prepared { places: self.places, added: false, changed: true }
    }

    fn place(&mut self, cx: &mut PlacementContext<'_, Mark<OTHER>>, _groups: CollisionGroups, _fade: f32) -> Placed {
        self.calls.push(format!("place {}", cx.layers.len()));
        Placed { changed: false, settled: true }
    }

    fn update_buckets(&mut self, cx: &mut PlacementContext<'_, Mark<OTHER>>, _changed: &mut ChangedBuckets) {
        self.calls.push(format!("update_buckets {}", cx.layers.len()));
    }

    fn forget_buckets(&mut self) {
        self.calls.push("forget_buckets".to_owned());
    }

    fn reset(&mut self) {
        self.calls.push("reset".to_owned());
    }

    fn query_rendered_features(&self, _query: &mut PlacementQuery<'_>) -> Result<(), Error> {
        Ok(())
    }
}
