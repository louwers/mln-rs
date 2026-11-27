//! A layer type as the engine holds it, whichever it is: the registry holds
//! each [`Layer`] as `&'static dyn ErasedLayerType` ([`entry`]).
//!
//! A layer's values for a frame are an [`EvaluatedLayer<S>`] of its style
//! half `S`, shared as an `Arc<dyn ErasedEvaluatedLayer>`. An erased value
//! gets back to its type only through [`EvaluatedLayer::recover`].

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use mln_style::CrossfadeParameters;
use mln_style::collections::VecSet;
use mln_style::error::Error;
use mln_style::expression::Dependency;
use mln_style::layer::{ErasedLayerStyle, ErasedStyleLayerType, LayerStyle, PaintSet, StyleLayerType, style_entry};
use mln_style::wire::SharedLayer;
use mln_wire::{Reader, Wire, Writer, shared};
use smallvec::SmallVec;

use crate::layer::bucket::{ErasedBucket, Stored};
use crate::layer::bucket_parameters::BucketParameters;
use crate::layer::layout::{BuildCx, Layout, LayoutCx, LayoutParameters, SourceLayer, layers_by_id};
use crate::layer::render_layer::{Hosted, Layer, RenderLayer};
use crate::layer::tile_input::input::Find;
use crate::layer::tile_layout::{Built, Resolved};
use crate::tile_store::contents::GeometryContents;

/// The values of a layer of the style half `T` for a frame.
pub struct EvaluatedLayer<T: StyleLayerType> {
    pub(crate) style: Arc<LayerStyle<T>>,
    pub(crate) paint: <T::PaintProperties as PaintSet>::Evaluated,
    /// The default for a type without cross-faded properties.
    pub(crate) crossfade: CrossfadeParameters,
    /// The `RenderPass` bits the layer draws in.
    pub(crate) render_passes: u8,
    /// The layer type whose layer has the values.
    pub(crate) layer_type: &'static dyn ErasedLayerType,
}

impl<T: StyleLayerType> EvaluatedLayer<T> {
    /// `layer` as a `T` layer's values, the same allocation, or `None` for
    /// another type's.
    pub(crate) fn recover(layer: &Arc<dyn ErasedEvaluatedLayer>) -> Option<Arc<Self>> {
        let any: Arc<dyn Any + Send + Sync> = Arc::<dyn ErasedEvaluatedLayer>::clone(layer);
        any.downcast().ok()
    }

    /// The values of the layer `style` of `layer_type`, before evaluation.
    pub(crate) fn new(style: Arc<LayerStyle<T>>, layer_type: &'static dyn ErasedLayerType) -> Self {
        EvaluatedLayer::with_paint(style, Default::default(), layer_type)
    }

    /// The values of the layer `style` of `layer_type` with its paint
    /// properties evaluated to `paint`, without a cross-fade or render
    /// passes.
    pub(crate) fn with_paint(
        style: Arc<LayerStyle<T>>,
        paint: <T::PaintProperties as PaintSet>::Evaluated,
        layer_type: &'static dyn ErasedLayerType,
    ) -> Self {
        EvaluatedLayer { style, paint, crossfade: CrossfadeParameters::default(), render_passes: 0, layer_type }
    }
}

impl<T: StyleLayerType> EvaluatedLayer<T> {
    /// The layer's style.
    #[must_use]
    pub fn style(&self) -> &Arc<LayerStyle<T>> {
        &self.style
    }

    /// The layer's paint values for the frame, of which a data-driven one
    /// can still differ by feature.
    #[must_use]
    pub fn paint(&self) -> &<T::PaintProperties as PaintSet>::Evaluated {
        &self.paint
    }

    /// The frame's cross-fade of the layer's cross-faded properties, the
    /// default for a type without them ([`Layer::CROSSFADES`]).
    #[must_use]
    pub fn crossfade(&self) -> CrossfadeParameters {
        self.crossfade
    }
}

#[cfg(test)]
impl<T: StyleLayerType> EvaluatedLayer<T> {
    /// [`new`](Self::new), of the layer type of the style half `T` that the
    /// tests register.
    pub(crate) fn registered(style: Arc<LayerStyle<T>>) -> Self {
        Self::new(style, Self::type_of(&crate::tests::registry()))
    }

    /// [`new`](Self::new), of the built-in layer type of the style half `T`.
    pub(crate) fn builtin(style: Arc<LayerStyle<T>>) -> Self {
        Self::new(style, Self::type_of(&crate::tests::builtin_registry()))
    }

    /// [`with_paint`](Self::with_paint), of the built-in layer type of the
    /// style half `T`.
    pub(crate) fn builtin_with_paint(
        style: Arc<LayerStyle<T>>,
        paint: <T::PaintProperties as PaintSet>::Evaluated,
    ) -> Self {
        Self::with_paint(style, paint, Self::type_of(&crate::tests::builtin_registry()))
    }

    fn type_of(registry: &crate::Registry) -> &'static dyn ErasedLayerType {
        registry.layer_type(T::INFO.name).expect("a layer type of the registry")
    }
}

impl<T: StyleLayerType> std::fmt::Debug for EvaluatedLayer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvaluatedLayer")
            .field("type", &T::INFO.name)
            .field("id", &self.style.id)
            .field("render_passes", &self.render_passes)
            .finish_non_exhaustive()
    }
}

/// A layer's values for a frame, whichever its type.
pub(crate) trait ErasedEvaluatedLayer: Any + Send + Sync {
    fn entry(&self) -> &'static dyn ErasedLayerType;

    fn style(&self) -> &dyn ErasedLayerStyle;

    fn shared_style(&self) -> Arc<dyn ErasedLayerStyle>;

    /// Bit i is set when the i-th data-driven paint property is the same for
    /// every feature.
    fn constants_mask(&self) -> u64;

    /// The combined dependencies of the evaluated paint properties that still
    /// differ by feature.
    fn evaluated_dependencies(&self) -> Dependency;

    /// Adds the global-state keys that the evaluated paint properties still
    /// read to `refs`.
    fn collect_evaluated_global_state_refs(&self, refs: &mut VecSet<String>);

    /// Writes the values for their type's [`ErasedLayerType::read_evaluated`].
    fn write(&self, w: &mut Writer);
}

impl<T: StyleLayerType> ErasedEvaluatedLayer for EvaluatedLayer<T> {
    fn entry(&self) -> &'static dyn ErasedLayerType {
        self.layer_type
    }

    fn style(&self) -> &dyn ErasedLayerStyle {
        &*self.style
    }

    fn shared_style(&self) -> Arc<dyn ErasedLayerStyle> {
        Arc::<LayerStyle<T>>::clone(&self.style)
    }

    fn constants_mask(&self) -> u64 {
        T::PaintProperties::constants_mask(&self.paint)
    }

    fn evaluated_dependencies(&self) -> Dependency {
        T::PaintProperties::evaluated_dependencies(&self.paint)
    }

    fn collect_evaluated_global_state_refs(&self, refs: &mut VecSet<String>) {
        T::PaintProperties::collect_evaluated_global_state_refs(&self.paint, refs);
    }

    fn write(&self, w: &mut Writer) {
        shared::write::<SharedLayer>(&self.shared_style(), w);
        self.paint.write(w);
        self.crossfade.write(w);
        self.render_passes.write(w);
    }
}

// Debug is not a supertrait, for the reason at mln_style's
// `impl Debug for dyn ErasedLayerStyle`.
impl std::fmt::Debug for dyn ErasedEvaluatedLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvaluatedLayer")
            .field("id", &self.style().base().id)
            .field("type", &self.style().info().name)
            .finish_non_exhaustive()
    }
}

/// A layer type that the renderer draws, whichever it is.
pub(crate) trait ErasedLayerType: Sync {
    /// The type's style half.
    fn style(&self) -> &'static dyn ErasedStyleLayerType;

    /// See [`Layer::FADES_TILES`].
    fn fades_tiles(&self) -> bool;

    /// See [`Layer::PLACES`].
    fn places(&self) -> bool;

    /// The render layer that draws the layer `style`, or `None` if `style`
    /// is another type's.
    fn create_render_layer(&self, style: &Arc<dyn ErasedLayerStyle>) -> Option<Box<dyn RenderLayer>>;

    /// [`Layout::needs_resources`] of the type's layout; false for a layer
    /// of another type or a type that doesn't lay out.
    fn needs_resources(&self, group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool;

    /// The type's layout of `group`, layers of the same layout on
    /// `source_layer`, with the dependencies it records in `parameters`;
    /// `None` for a type that doesn't lay out. Fails for a layer of another
    /// type.
    fn create_layout(
        &self,
        group: &[Arc<dyn ErasedEvaluatedLayer>],
        parameters: LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<Box<dyn ErasedLayout>>, Error>;

    /// [`Layer::query_radius`] of the bucket that `contents` laid out
    /// for `layer`; 0 where they laid out none of the type's.
    fn query_radius(&self, contents: &GeometryContents, layer: &Arc<dyn ErasedEvaluatedLayer>) -> f32;

    /// Reads the values that [`ErasedEvaluatedLayer::write`] wrote for a
    /// layer of the type.
    fn read_evaluated(&self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>>;

    /// Reads a bucket of the type, which [`ErasedBucket::write_wire`] wrote.
    fn read_bucket(&self, r: &mut Reader<'_>) -> Option<Box<dyn ErasedBucket>>;
}

/// The [`ErasedLayerType`] of `L`, which [`entry`] returns.
struct LayerEntry<L>(PhantomData<fn() -> L>);

impl<L: Layer> LayerEntry<L> {
    const INSTANCE: &'static Self = &LayerEntry(PhantomData);
}

impl<L: Layer> ErasedLayerType for LayerEntry<L> {
    fn style(&self) -> &'static dyn ErasedStyleLayerType {
        style_entry::<L::Style>()
    }

    fn fades_tiles(&self) -> bool {
        L::FADES_TILES
    }

    fn places(&self) -> bool {
        L::PLACES
    }

    fn create_render_layer(&self, style: &Arc<dyn ErasedLayerStyle>) -> Option<Box<dyn RenderLayer>> {
        LayerStyle::<L::Style>::recover(style).map(Hosted::<L>::boxed)
    }

    fn needs_resources(&self, group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool {
        <L::Input as Find<L::Style>>::needs_resources(group)
    }

    fn create_layout(
        &self,
        group: &[Arc<dyn ErasedEvaluatedLayer>],
        parameters: LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<Box<dyn ErasedLayout>>, Error> {
        <L::Input as Find<L::Style>>::create_layout(group, parameters, source_layer)
    }

    fn query_radius(&self, contents: &GeometryContents, layer: &Arc<dyn ErasedEvaluatedLayer>) -> f32 {
        let Some(layer) = EvaluatedLayer::<L::Style>::recover(layer) else {
            return 0.0;
        };
        <L::Input as Find<L::Style>>::laid(contents, &layer.style.id).map_or(0.0, |data| L::query_radius(data, &layer))
    }

    fn read_evaluated(&self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>> {
        let style = shared::read::<SharedLayer>(r)?;
        let Some(style) = LayerStyle::<L::Style>::recover(&style) else {
            return r.fail("the evaluated values of a layer of another type");
        };
        Some(Arc::new(EvaluatedLayer::<L::Style> {
            style,
            paint: Wire::read(r)?,
            crossfade: Wire::read(r)?,
            render_passes: Wire::read(r)?,
            layer_type: entry::<L>(),
        }))
    }

    fn read_bucket(&self, r: &mut Reader<'_>) -> Option<Box<dyn ErasedBucket>> {
        <L::Input as Find<L::Style>>::read_bucket(r)
    }
}

/// [`Layout::needs_resources`] of `L`, a layout of the style half `T`, for
/// `group`; false if a layer of it is another type's.
pub(crate) fn needs_resources<T: StyleLayerType, L: Layout<T>>(group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool {
    recover_group::<T>(group).is_ok_and(|group| L::needs_resources(&group))
}

/// The layout `L` of `group`, layers of the type `T` of the same layout on
/// `source_layer`, with the dependencies it records in `parameters`. Fails
/// for a layer of another type.
pub(crate) fn create_layout<T: StyleLayerType, L: Layout<T>>(
    group: &[Arc<dyn ErasedEvaluatedLayer>],
    parameters: LayoutParameters<'_>,
    source_layer: SourceLayer,
) -> Result<Box<dyn ErasedLayout>, Error> {
    let layers = recover_group::<T>(group)?;
    debug_assert!(!layers.is_empty());
    let needs_resources = L::needs_resources(&layers);
    let layout = L::new(&mut LayoutCx { layers: &layers, source_layer: &source_layer, parameters })?;
    Ok(Box::new(TypedLayout::<T, L> { layout, layers, source_layer, needs_resources }))
}

/// `group`'s layers as `T` layers' values; fails if one is another type's.
// Inline for small groups, since a tile's layout recovers each of its groups.
fn recover_group<T: StyleLayerType>(
    group: &[Arc<dyn ErasedEvaluatedLayer>],
) -> Result<SmallVec<[Arc<EvaluatedLayer<T>>; 4]>, Error> {
    group
        .iter()
        .map(|layer| {
            EvaluatedLayer::recover(layer).ok_or_else(|| {
                Error::native(format!("a {} layer in a group of {} layers", layer.style().info().name, T::INFO.name))
            })
        })
        .collect()
}

/// A layout, whichever its layer type.
pub(crate) trait ErasedLayout: Send {
    /// [`Layout::needs_resources`] of the layout's group: whether its
    /// bucket waits for the glyphs and images it asked for.
    fn needs_resources(&self) -> bool;

    /// Makes the bucket ([`Layout::build`]) of the tile `parameters`
    /// describe and, if there is one, adds it to `built` for each of its
    /// layers that has none yet. `resolved` is what the dependencies
    /// resolved to, for a layout that waited for them.
    fn build(
        self: Box<Self>,
        resolved: Option<&Resolved>,
        built: &mut Built,
        parameters: &BucketParameters,
        first_load: bool,
    ) -> Result<(), Error>;
}

/// A layout with the group it lays out and the layer it reads.
struct TypedLayout<T: StyleLayerType, L> {
    layout: L,
    layers: SmallVec<[Arc<EvaluatedLayer<T>>; 4]>,
    source_layer: SourceLayer,
    needs_resources: bool,
}

impl<T: StyleLayerType, L: Layout<T>> ErasedLayout for TypedLayout<T, L> {
    fn needs_resources(&self) -> bool {
        self.needs_resources
    }

    fn build(
        self: Box<Self>,
        resolved: Option<&Resolved>,
        built: &mut Built,
        parameters: &BucketParameters,
        first_load: bool,
    ) -> Result<(), Error> {
        let TypedLayout { layout, layers, source_layer, .. } = *self;
        let mut cx = BuildCx {
            layers: &layers,
            parameters,
            first_load,
            source_layer: &source_layer,
            resolved,
            feature_index: &mut built.feature_index,
        };
        let Some(bucket) = layout.build(&mut cx)? else {
            return Ok(());
        };
        let layers = layers_by_id(&layers)
            .into_iter()
            .map(|(id, layer)| -> (String, Arc<dyn ErasedEvaluatedLayer>) { (id, layer) });
        built.buckets.add(Box::new(Stored::new(bucket)), layers);
        Ok(())
    }
}

/// The layer type `L` as a value.
pub(crate) const fn entry<L: Layer>() -> &'static dyn ErasedLayerType {
    LayerEntry::<L>::INSTANCE
}

#[cfg(test)]
mod tests {
    use mln_style::color::Color;
    use mln_style::layers::{Background, BackgroundPaintProperties, Circle};
    use mln_style::property::evaluated::Evaluated;

    use super::*;
    use crate::symbol::layer::RenderSymbolLayer;
    use crate::tests::types::layers::{TestBackground, TestFeatures};

    fn background() -> Arc<dyn ErasedEvaluatedLayer> {
        let paint = BackgroundPaintProperties::PossiblyEvaluated {
            background_color: Evaluated::Constant(Color::new(0.25, 0.5, 0.75, 1.0)),
            background_opacity: Evaluated::Constant(0.5),
            ..BackgroundPaintProperties::PossiblyEvaluated::default()
        };
        let style = Arc::new(LayerStyle::<Background>::new("background".to_owned(), String::new()));
        let crossfade = CrossfadeParameters { from_scale: 2.0, to_scale: 1.0, t: 0.25 };
        let layer_type = entry::<TestBackground>();
        Arc::new(EvaluatedLayer { style, paint, crossfade, render_passes: 0, layer_type })
    }

    #[test]
    fn recover_keeps_the_allocation() {
        let layer = background();
        let recovered = EvaluatedLayer::<Background>::recover(&layer).unwrap();
        assert!(std::ptr::addr_eq(Arc::as_ptr(&recovered), Arc::as_ptr(&layer)));
        assert_eq!(*recovered.paint.background_color.uniform(), Color::new(0.25, 0.5, 0.75, 1.0));
        assert_eq!(*recovered.paint.background_opacity.uniform(), 0.5);
        assert_eq!(recovered.crossfade.t, 0.25);
        assert!(EvaluatedLayer::<Circle>::recover(&layer).is_none());
    }

    #[test]
    fn a_type_reads_no_other_types_values() {
        let layer = background();
        let circles = entry::<TestFeatures<Circle>>();
        assert!(!circles.needs_resources(std::slice::from_ref(&layer)));
        let symbols = entry::<RenderSymbolLayer>();
        assert!(!symbols.needs_resources(&[layer]));
    }
}
