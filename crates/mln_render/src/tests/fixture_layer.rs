//! Layer types for the tests of this crate, which can't reach the built-in
//! layer types outside it.
//!
//! `dot` is a layer type of a geometry source. Its layout makes a bucket of
//! each feature's points that its filter selects, with each point's
//! `dot-color`, which can read feature state, and adds the features to the
//! tile's feature index. A layer with a `dot-pattern` needs the pattern's
//! image, as the layer types that draw patterns do, and one with a
//! `dot-icon` or a `dot-label` the icon of each feature and the glyphs of
//! its label in the font stack `dot-font`, as layer types that draw icons
//! and text do. `dot-size` changes nothing but the layout.
//!
//! `tint` is a layer type of a raster or image source, with a constant
//! `tint-opacity`, and `shade` one of a raster-dem source without
//! properties.
//!
//! `mark` is a layer type without a source or properties whose layers
//! place, with a [`MarkPass`] that places nothing and records the engine's
//! calls; `other-mark` is another. A registry has at most one of them.
//!
//! None of them draws anything.

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
    /// The layout properties of a `dot` layer.
    #[expect(non_snake_case, reason = "named as the layer types' property sets are")]
    pub(crate) mod DotLayoutProperties: layout {
        dot_font: data_driven Vec<String> = Vec::new(),
        dot_icon: data_driven Image = Image::default(),
        dot_label: data_driven Formatted = Formatted::default(),
        dot_size: data_driven f32 = 1.0_f32,
    }
    use mln_style::expression::Formatted, mln_style::expression::Image;
);

mln_style::properties!(
    /// The paint properties of a `dot` layer.
    #[expect(non_snake_case, reason = "named as the layer types' property sets are")]
    pub(crate) mod DotPaintProperties: paint {
        dot_color: data_driven Color = Color::black(),
        dot_pattern: cross_faded Image = Image::default(),
    }
    use mln_style::expression::Image, mln_util::Color;
);

static PROPERTIES: PropertyTable = PropertyTable::new(
    &[
        PropertyInfo::paint("dot-color").data_driven(),
        PropertyInfo::paint("dot-pattern"),
        PropertyInfo::layout("dot-font").data_driven(),
        PropertyInfo::layout("dot-icon").data_driven(),
        PropertyInfo::layout("dot-label").data_driven(),
        PropertyInfo::layout("dot-size").data_driven(),
    ],
    &[0, 2, 3, 4, 1, 5],
);

/// The image of `layer`'s `dot-pattern`, if it has one.
fn pattern(layer: &EvaluatedLayer<Dot>) -> Option<&str> {
    Some(layer.paint.dot_pattern.faded().to.id()).filter(|id| !id.is_empty())
}

/// Whether `layer` has an icon or a label, whose layout needs their images
/// and glyphs.
fn draws_icons_or_labels(layer: &EvaluatedLayer<Dot>) -> bool {
    let layout = &layer.style.layout;
    !layout.dot_icon.is_undefined() || !layout.dot_label.is_undefined()
}

/// The `dot` layer type.
pub(crate) struct Dot;

impl StyleLayerType for Dot {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "dot", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = DotLayoutProperties::Unevaluated;
    type PaintProperties = DotPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        Some(match slot {
            0 => visitor.paint(|s| &s.paint.dot_color, |s| &mut s.paint.dot_color),
            1 => visitor.paint(|s| &s.paint.dot_pattern, |s| &mut s.paint.dot_pattern),
            2 => visitor.layout(|s| &s.layout.dot_font, |s| &mut s.layout.dot_font),
            3 => visitor.layout(|s| &s.layout.dot_icon, |s| &mut s.layout.dot_icon),
            4 => visitor.layout(|s| &s.layout.dot_label, |s| &mut s.layout.dot_label),
            5 => visitor.layout(|s| &s.layout.dot_size, |s| &mut s.layout.dot_size),
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
        let Some(binders) = self.binders.get_mut(layer_id).filter(|binders| binders.depends_on_feature_state()) else {
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
    /// Whether a layer of the group has a pattern, an icon or a label.
    has_dependencies: bool,
}

impl Layout<Dot> for DotLayout {
    fn has_dependencies(&self) -> bool {
        self.has_dependencies
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
    type Tile = ();
    type Drawing = TileDraws;
    type State = ();

    fn layout_can_need_dependencies(group: &[Arc<EvaluatedLayer<Dot>>]) -> bool {
        group.iter().any(|layer| pattern(layer).is_some() || draws_icons_or_labels(layer))
    }

    fn create_layout(
        group: &[Arc<EvaluatedLayer<Dot>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<DotLayout>, Error> {
        let mut has_dependencies = draws_icons_or_labels(&group[0]);
        for id in group.iter().filter_map(|layer| pattern(layer)) {
            parameters.image_dependencies.entry(id.to_owned()).or_insert(ImageType::Pattern);
            has_dependencies = true;
        }
        let leader = &group[0].style;
        let bucket_parameters = parameters.bucket_parameters;
        let zoom = f32::from(bucket_parameters.tile_id.overscaled_z);
        let context = EvaluationContext::new(zoom)
            .with_canonical_tile_id(Some(&bucket_parameters.tile_id.canonical))
            .with_global_state(bucket_parameters.global_state.as_deref());
        let mut evaluation = PropertyEvaluationParameters::new(zoom);
        evaluation.global_state.clone_from(&bucket_parameters.global_state);
        let layout = leader.layout.evaluate(&evaluation);
        let layer = source_layer.layer()?;
        let mut features = Vec::new();
        for i in 0..layer.len() {
            let feature = layer.feature(i)?;
            let context = EvaluationContext { feature: Some(&feature), ..context };
            if !leader.filter.evaluate(&context) {
                continue;
            }
            features.push(i);
            if !draws_icons_or_labels(&group[0]) {
                continue;
            }
            let images = context
                .with_available_images(Some(parameters.available_images))
                .with_unavailable_images(Some(parameters.unavailable_images));
            let icon = layout.dot_icon.evaluate(&images);
            if !icon.empty() {
                parameters.image_dependencies.entry(icon.id().to_owned()).or_insert(ImageType::Icon);
            }
            if !leader.layout.dot_label.is_undefined() {
                let font_stack = layout.dot_font.evaluate(&context);
                let glyphs = parameters.glyph_dependencies.glyphs.entry(font_stack).or_default();
                for section in &layout.dot_label.evaluate(&images).sections {
                    glyphs.extend(section.text.chars().map(|c| GlyphID::from(c as u16)));
                }
            }
        }
        Ok(Some(DotLayout {
            layers: layers_by_id(group),
            leader_id: leader.id.clone(),
            source_layer_id: leader.source_layer.clone(),
            zoom,
            features,
            source_layer,
            has_dependencies,
        }))
    }

    fn new_layer(_layer: &LayerValues<Dot>) -> ((), TileDraws) {
        Default::default()
    }

    fn evaluate((): &mut (), _cx: EvaluateCx<'_, Dot>) -> Evaluation {
        Evaluation::TRANSLUCENT
    }

    fn make_draws(
        _state: &mut (),
        _drawing: &mut TileDraws,
        _tile_values: &mut TileMap<()>,
        _gpu: &mut dyn mln_gfx::gpu::Gpu,
        _cx: &mut DrawContext<'_, '_, Dot>,
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
