//! Layer types for the tests of this crate, which can't reach the built-in
//! layer types outside it.
//!
//! `dot` is a layer type of a geometry source. Its layout makes a bucket of
//! each feature's points that its filter selects, with each point's
//! `dot-color`, which can read feature state, and adds the features to the
//! tile's feature index. A layer with a `dot-pattern` needs the pattern's
//! image, as the layer types that draw patterns do.
//!
//! `tint` is a layer type of a raster or image source, with a constant
//! `tint-opacity`, and `shade` one of a raster-dem source without
//! properties.
//!
//! None of them draws anything.

use std::sync::Arc;

use mln_style::expression::{EvaluationContext, Value};
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyInfo, PropertyTable, PropertyVisitor};
use mln_style_types::image::{ImagePositions, ImageType};
use mln_util::error::Error;
use mln_vec_map::VecMap;

use crate::layer::{
    Bucket, BucketBase, BuildParams, DemContents, DrawContext, EvaluatedLayer, FeatureIndexBuilder, FeatureStates,
    GeometryUpload, GeometryVec, LayerBucket, LayerType, LayerValues, Layout, LayoutParameters, NoBucket, NoLayout,
    RasterView, RenderPass, Resolved, SourceLayer, layers_by_id,
};
use crate::layer_drawing::{KeptDraw, TileDraws};

mln_style::properties!(
    /// The paint properties of a `dot` layer.
    #[expect(non_snake_case, reason = "named as the layer types' property sets are")]
    pub(crate) mod DotPaintProperties: paint {
        dot_color: data_driven Color = Color::black(),
        dot_pattern: cross_faded Image = Image::default(),
    }
    use mln_style::expression::Image, mln_util::Color;
);

static PROPERTIES: PropertyTable =
    PropertyTable::new(&[PropertyInfo::paint("dot-color").data_driven(), PropertyInfo::paint("dot-pattern")], &[0, 1]);

/// The image of `layer`'s `dot-pattern`, if it has one.
fn pattern(layer: &EvaluatedLayer<Dot>) -> Option<&str> {
    Some(layer.paint.dot_pattern.faded().to.id()).filter(|id| !id.is_empty())
}

/// The `dot` layer type.
pub(crate) struct Dot;

impl StyleLayerType for Dot {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "dot", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = DotPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        Some(match slot {
            0 => visitor.paint(|s| &s.paint.dot_color, |s| &mut s.paint.dot_color),
            1 => visitor.paint(|s| &s.paint.dot_pattern, |s| &mut s.paint.dot_pattern),
            _ => return None,
        })
    }
}

#[expect(dead_code, reason = "a `dot` layer draws nothing, so it binds none of its binders")]
mod binders {
    use mln_util::Color;

    use super::DotPaintProperties;
    use crate::layer::PaintPropertyBinder;

    crate::paint_property_binders!(
        pub(crate) struct DotBinders for DotPaintProperties::PossiblyEvaluated {
            dot_color: PaintPropertyBinder<Color> => [0],
        }
    );
}

use binders::DotBinders;

/// A `dot` layer's bucket: the points of its features.
#[derive(Clone, Default, mln_wire::Wire)]
pub(crate) struct DotBucket {
    base: BucketBase,
    pub(crate) vertices: GeometryVec<[i16; 2]>,
    binders: VecMap<String, DotBinders>,
}

impl Bucket for DotBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

    fn update(
        &mut self,
        states: &FeatureStates,
        layer: &mln_tile::TileLayer<'_>,
        layer_id: &str,
        image_positions: &ImagePositions,
    ) -> bool {
        let Some(binders) = self.binders.get_mut(layer_id) else {
            return false;
        };
        binders.update_vertex_vectors(states, layer, image_positions);
        self.base.set_uploaded(false);
        true
    }

    fn upload(&mut self, geometry: &mut GeometryUpload<'_>) {
        geometry.move_vertices(&mut self.vertices);
        for binders in self.binders.values_mut() {
            let depends_on_feature_state = binders.depends_on_feature_state();
            geometry.move_paint(&mut binders.interleaved_vertex_buffer, depends_on_feature_state);
        }
        self.base.set_uploaded(true);
    }

    fn has_data(&self) -> bool {
        !self.vertices.is_empty()
    }
}

/// A `dot` group's layout: the features its leader's filter selects, by
/// index in the source layer.
pub(crate) struct DotLayout {
    layers: VecMap<String, Arc<EvaluatedLayer<Dot>>>,
    leader_id: String,
    source_layer_id: String,
    zoom: f32,
    features: Vec<usize>,
    source_layer: SourceLayer,
    /// Whether a layer of the group has a pattern.
    has_pattern: bool,
}

impl Layout<Dot> for DotLayout {
    fn has_dependencies(&self) -> bool {
        self.has_pattern
    }

    fn create_bucket(
        &mut self,
        _resolved: Option<&Resolved>,
        feature_index: &mut FeatureIndexBuilder,
        build: &BuildParams,
    ) -> Result<Option<DotBucket>, Error> {
        let layer = self.source_layer.layer()?;
        let binders = self.layers.iter().map(|(id, layer)| (id.clone(), DotBinders::new(&layer.paint, self.zoom)));
        let mut bucket =
            DotBucket { base: BucketBase::default(), vertices: GeometryVec::default(), binders: binders.collect() };
        for &i in &self.features {
            let feature = layer.feature(i)?;
            let geometry = feature.geometry()?;
            bucket.vertices.get_mut().extend(geometry.iter().flatten().map(|p| [p.x, p.y]));
            let length = bucket.vertices.len();
            for binders in bucket.binders.values_mut() {
                let (positions, canonical) = (&ImagePositions::default(), &build.canonical);
                binders.populate_vertex_vectors(&feature, length, i, positions, None, canonical, &Value::Null);
            }
            feature_index.insert(&geometry, i, &self.source_layer_id, &self.leader_id);
        }
        Ok(Some(bucket))
    }

    fn layers(&self) -> &VecMap<String, Arc<EvaluatedLayer<Dot>>> {
        &self.layers
    }
}

impl LayerType for Dot {
    type Layout = DotLayout;
    type Bucket = DotBucket;
    type View<'a> = LayerBucket<'a, Dot>;
    type Drawing = TileDraws<KeptDraw>;
    type State = ();

    fn layout_can_need_dependencies(group: &[Arc<EvaluatedLayer<Dot>>]) -> bool {
        group.iter().any(|layer| pattern(layer).is_some())
    }

    fn create_layout(
        group: &[Arc<EvaluatedLayer<Dot>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<DotLayout>, Error> {
        let mut has_pattern = false;
        for id in group.iter().filter_map(|layer| pattern(layer)) {
            parameters.image_dependencies.entry(id.to_owned()).or_insert(ImageType::Pattern);
            has_pattern = true;
        }
        let leader = &group[0].style;
        let bucket_parameters = parameters.bucket_parameters;
        let zoom = f32::from(bucket_parameters.tile_id.overscaled_z);
        let context = EvaluationContext::new(zoom)
            .with_canonical_tile_id(Some(&bucket_parameters.tile_id.canonical))
            .with_global_state(bucket_parameters.global_state.as_deref());
        let layer = source_layer.layer()?;
        let mut features = Vec::new();
        for i in 0..layer.len() {
            let feature = layer.feature(i)?;
            if leader.filter.evaluate(&EvaluationContext { feature: Some(&feature), ..context }) {
                features.push(i);
            }
        }
        Ok(Some(DotLayout {
            layers: layers_by_id(group),
            leader_id: leader.id.clone(),
            source_layer_id: leader.source_layer.clone(),
            zoom,
            features,
            source_layer,
            has_pattern,
        }))
    }

    fn new_layer(_layer: &LayerValues<Dot>) -> ((), TileDraws<KeptDraw>) {
        Default::default()
    }

    fn passes(_layer: &LayerValues<Dot>, _evaluated: &EvaluatedLayer<Dot>) -> RenderPass {
        RenderPass::TRANSLUCENT
    }

    fn make_draws(
        _state: &mut (),
        _drawing: &mut TileDraws<KeptDraw>,
        _gpu: &mut dyn mln_gfx::gpu::Gpu,
        _cx: &mut DrawContext<'_, '_, '_, Dot>,
    ) {
    }
}

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
    type Drawing = TileDraws<KeptDraw>;
    type State = ();

    fn new_layer(_layer: &LayerValues<Tint>) -> ((), TileDraws<KeptDraw>) {
        Default::default()
    }

    fn passes(_layer: &LayerValues<Tint>, _evaluated: &EvaluatedLayer<Tint>) -> RenderPass {
        RenderPass::TRANSLUCENT
    }

    fn make_draws(
        _state: &mut (),
        _drawing: &mut TileDraws<KeptDraw>,
        _gpu: &mut dyn mln_gfx::gpu::Gpu,
        _cx: &mut DrawContext<'_, '_, '_, Tint>,
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
    type Drawing = TileDraws<KeptDraw>;
    type State = ();

    fn new_layer(_layer: &LayerValues<Shade>) -> ((), TileDraws<KeptDraw>) {
        Default::default()
    }

    fn passes(_layer: &LayerValues<Shade>, _evaluated: &EvaluatedLayer<Shade>) -> RenderPass {
        RenderPass::TRANSLUCENT
    }

    fn make_draws(
        _state: &mut (),
        _drawing: &mut TileDraws<KeptDraw>,
        _gpu: &mut dyn mln_gfx::gpu::Gpu,
        _cx: &mut DrawContext<'_, '_, '_, Shade>,
    ) {
    }
}
