//! The layer types as the engine holds them: the registry holds each as
//! `&'static dyn ErasedLayerType` ([`entry`]).
//!
//! A layer's values for a frame are an [`EvaluatedLayer<S>`] of its style
//! type, shared as an `Arc<dyn ErasedEvaluatedLayer>`. An erased value gets
//! back to its type only through [`EvaluatedLayer::recover`].

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use mln_style::CrossfadeParameters;
use mln_style::expression::Dependency;
use mln_style::layer_type::{
    ErasedLayerStyle, ErasedStyleLayerType, LayerStyle, LayerTypeEntry, PaintSet, StyleLayerType,
};
use mln_style::serialization::SharedLayer;
use mln_util::error::Error;
use mln_util::serialization::shared;
use mln_vec_map::VecSet;
use mln_wire::{Reader, Wire, Writer};
use smallvec::SmallVec;

use crate::bucket::{ErasedBucket, Stored};
use crate::layer::{Features, Layer, LayerBucket, Layout};
use crate::layout::layout::{BuildParams, LayoutParameters, SourceLayer};
use crate::layout::tile_layout::{Built, Resolved};
use crate::placement::ErasedPlacement;
use crate::render_layer::{Hosted, RenderLayer};

/// The values of a layer of the style type `S` for a frame.
pub struct EvaluatedLayer<S: StyleLayerType> {
    /// The layer's style, which its layout properties are part of.
    pub style: Arc<LayerStyle<S>>,
    /// The paint properties, evaluated for the frame.
    pub paint: <S::PaintProperties as PaintSet>::Evaluated,
    /// The frame's cross-fade between a cross-faded property's values; the
    /// default for a type without cross-faded properties.
    pub crossfade: CrossfadeParameters,
    /// Whether the layer draws with these values
    /// ([`Evaluation::draws`](crate::layer::Evaluation::draws)).
    pub(crate) draws: bool,
    /// The layer's type.
    pub(crate) entry: &'static dyn ErasedLayerType,
}

impl<S: StyleLayerType> EvaluatedLayer<S> {
    /// `layer` as an `S` layer's values, the same allocation, or `None` for
    /// another type's.
    pub(crate) fn recover(layer: &Arc<dyn ErasedEvaluatedLayer>) -> Option<Arc<Self>> {
        let any: Arc<dyn Any + Send + Sync> = Arc::<dyn ErasedEvaluatedLayer>::clone(layer);
        any.downcast().ok()
    }

    /// The values of the layer `style` of the type `entry` with its paint
    /// properties evaluated to `paint`, without a cross-fade, not drawing.
    pub(crate) fn with_paint(
        entry: &'static dyn ErasedLayerType,
        style: Arc<LayerStyle<S>>,
        paint: <S::PaintProperties as PaintSet>::Evaluated,
    ) -> Self {
        EvaluatedLayer { style, paint, crossfade: CrossfadeParameters::default(), draws: false, entry }
    }
}

#[cfg(test)]
impl<S: Layer<Style = S> + StyleLayerType> EvaluatedLayer<S> {
    /// The values of the layer `style` of the type `S`, which is both its
    /// halves, before its first evaluation: its paint properties' defaults,
    /// not drawing.
    pub(crate) fn new(style: Arc<LayerStyle<S>>) -> Self {
        EvaluatedLayer::with_paint(entry::<S>(), style, Default::default())
    }
}

impl<S: StyleLayerType> std::fmt::Debug for EvaluatedLayer<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvaluatedLayer")
            .field("type", &S::INFO.name)
            .field("id", &self.style.id)
            .field("draws", &self.draws)
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

impl<S: StyleLayerType> ErasedEvaluatedLayer for EvaluatedLayer<S> {
    fn entry(&self) -> &'static dyn ErasedLayerType {
        self.entry
    }

    fn style(&self) -> &dyn ErasedLayerStyle {
        &*self.style
    }

    fn shared_style(&self) -> Arc<dyn ErasedLayerStyle> {
        Arc::<LayerStyle<S>>::clone(&self.style)
    }

    fn constants_mask(&self) -> u64 {
        S::PaintProperties::constants_mask(&self.paint)
    }

    fn evaluated_dependencies(&self) -> Dependency {
        S::PaintProperties::evaluated_dependencies(&self.paint)
    }

    fn collect_evaluated_global_state_refs(&self, refs: &mut VecSet<String>) {
        S::PaintProperties::collect_evaluated_global_state_refs(&self.paint, refs);
    }

    fn write(&self, w: &mut Writer) {
        shared::write::<SharedLayer>(&self.shared_style(), w);
        self.paint.write(w);
        self.crossfade.write(w);
        self.draws.write(w);
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
pub(crate) trait ErasedLayerType: ErasedStyleLayerType {
    /// The render layer that draws the layer `style`, or `None` if `style`
    /// is another type's.
    fn create_render_layer(
        &self,
        style: &Arc<dyn ErasedLayerStyle>,
        res: &mut crate::gfx::context::Resources,
    ) -> Option<Box<dyn RenderLayer>>;

    /// [`Layout::needs_resources`]; false for a type that doesn't lay out,
    /// or a layer of another type.
    fn layout_can_need_dependencies(&self, group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool;

    /// The layout of `group`, layers of the type that share their layout,
    /// for a type that lays out. Fails for a layer of another type.
    fn create_layout(
        &self,
        group: &[Arc<dyn ErasedEvaluatedLayer>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<Box<dyn ErasedLayout>>, Error>;

    /// Whether the type's layers place.
    fn places(&self) -> bool;

    /// A new placement pass of the type's layers, for a type whose layers
    /// place.
    fn new_placement(&self) -> Option<Box<dyn ErasedPlacement>>;

    /// Reads the values that [`ErasedEvaluatedLayer::write`] wrote for a
    /// layer of the type.
    fn read_evaluated(&'static self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>>;

    /// Reads a bucket of the type, which [`ErasedBucket::write_wire`] wrote.
    fn read_bucket(&self, r: &mut Reader<'_>) -> Option<Box<dyn ErasedBucket>>;
}

/// The [`ErasedLayerType`] of the layer type `L`, which [`entry`] returns.
pub(crate) struct LayerEntry<L>(PhantomData<fn() -> L>);

impl<L: Layer> ErasedStyleLayerType for LayerEntry<L> {
    fn info(&self) -> mln_style::layer_type::LayerTypeInfo {
        L::Style::INFO
    }

    fn is_type_of(&self, style: &dyn ErasedLayerStyle) -> bool {
        style_half::<L>().is_type_of(style)
    }

    fn create(&self, id: &str, source: &str) -> Arc<dyn ErasedLayerStyle> {
        style_half::<L>().create(id, source)
    }

    fn set_property(
        &self,
        style: &mut Arc<dyn ErasedLayerStyle>,
        name: &str,
        value: &mln_style::conversion::Convertible<'_>,
        functions: &Arc<mln_style::expression::Functions>,
    ) -> Result<(), mln_style::property_table::SetPropertyError> {
        style_half::<L>().set_property(style, name, value, functions)
    }

    fn mutate_base(
        &self,
        style: &mut Arc<dyn ErasedLayerStyle>,
        f: &mut dyn FnMut(&mut mln_style::layer_type::LayerBase),
    ) {
        style_half::<L>().mutate_base(style, f);
    }

    fn read_style(&self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedLayerStyle>> {
        style_half::<L>().read_style(r)
    }

    fn keep_data(&self, style: &mut Arc<dyn ErasedLayerStyle>, from: &dyn ErasedLayerStyle) {
        style_half::<L>().keep_data(style, from);
    }
}

/// The style half of the layer type `L`.
fn style_half<L: Layer>() -> &'static LayerTypeEntry<L::Style> {
    LayerTypeEntry::<L::Style>::INSTANCE
}

impl<L: Layer> ErasedLayerType for LayerEntry<L> {
    fn create_render_layer(
        &self,
        style: &Arc<dyn ErasedLayerStyle>,
        res: &mut crate::gfx::context::Resources,
    ) -> Option<Box<dyn RenderLayer>> {
        let style = LayerStyle::<L::Style>::recover(style)?;
        Some(Box::new(Hosted::<L>::new(style, res)))
    }

    fn layout_can_need_dependencies(&self, group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool {
        <L::Input as LayoutOf<L::Style>>::needs_resources(group)
    }

    fn create_layout(
        &self,
        group: &[Arc<dyn ErasedEvaluatedLayer>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<Box<dyn ErasedLayout>>, Error> {
        <L::Input as LayoutOf<L::Style>>::create(group, parameters, source_layer)
    }

    fn places(&self) -> bool {
        false
    }

    fn new_placement(&self) -> Option<Box<dyn ErasedPlacement>> {
        None
    }

    fn read_evaluated(&'static self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>> {
        let style = shared::read::<SharedLayer>(r)?;
        let Some(style) = LayerStyle::<L::Style>::recover(&style) else {
            return r.fail("the evaluated values of a layer of another type");
        };
        Some(Arc::new(EvaluatedLayer::<L::Style> {
            style,
            paint: Wire::read(r)?,
            crossfade: Wire::read(r)?,
            draws: Wire::read(r)?,
            entry: self,
        }))
    }

    fn read_bucket(&self, r: &mut Reader<'_>) -> Option<Box<dyn ErasedBucket>> {
        <L::Input as LayoutOf<L::Style>>::read_bucket(r)
    }
}

/// What the engine does with a layer type's input on the tile workers: the
/// layout of a [`Features`] input, and nothing for the others.
pub(crate) trait LayoutOf<S: StyleLayerType> {
    /// [`Layout::needs_resources`].
    fn needs_resources(_group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool {
        false
    }

    /// The layout of `group`, for an input that lays out.
    fn create(
        _group: &[Arc<dyn ErasedEvaluatedLayer>],
        _parameters: &mut LayoutParameters<'_>,
        _source_layer: SourceLayer,
    ) -> Result<Option<Box<dyn ErasedLayout>>, Error> {
        Ok(None)
    }

    /// Reads a bucket, for an input that lays out.
    fn read_bucket(r: &mut Reader<'_>) -> Option<Box<dyn ErasedBucket>> {
        r.fail("a bucket of a layer type that doesn't lay out")
    }
}

impl<S: StyleLayerType, L: Layout<S>> LayoutOf<S> for Features<L> {
    fn needs_resources(group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool {
        recover_group::<S>(group).is_ok_and(|group| L::needs_resources(&group))
    }

    fn create(
        group: &[Arc<dyn ErasedEvaluatedLayer>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<Box<dyn ErasedLayout>>, Error> {
        let group = recover_group::<S>(group)?;
        let layout = crate::layout::layout::new_layout::<S, L>(&group, parameters, &source_layer)?;
        Ok(Some(Box::new(TypedLayout::<S, L> { layout, group, source_layer })))
    }

    fn read_bucket(r: &mut Reader<'_>) -> Option<Box<dyn ErasedBucket>> {
        Some(Box::new(Stored::new(L::Bucket::read(r)?)))
    }
}

impl<S: StyleLayerType> LayoutOf<S> for crate::layer::ViewCover {}

/// `group`'s layers as `S` layers' values; fails if one is another type's.
// Inline for small groups, since a tile's layout recovers each of its groups.
fn recover_group<S: StyleLayerType>(
    group: &[Arc<dyn ErasedEvaluatedLayer>],
) -> Result<SmallVec<[Arc<EvaluatedLayer<S>>; 4]>, Error> {
    group
        .iter()
        .map(|layer| {
            EvaluatedLayer::recover(layer).ok_or_else(|| {
                Error::native(format!("a {} layer in a group of {} layers", layer.style().info().name, S::INFO.name))
            })
        })
        .collect()
}

/// A layout, whichever its layer type.
pub(crate) trait ErasedLayout: Send {
    /// Whether the layout asked for glyphs or images.
    fn has_dependencies(&self) -> bool;

    /// Makes the bucket ([`Layout::build`]) and, if there is one, adds it to
    /// `built` for each of its layers that has none yet.
    fn create_bucket(
        self: Box<Self>,
        resolved: Option<&Resolved>,
        built: &mut Built,
        build: &BuildParams,
    ) -> Result<(), Error>;
}

struct TypedLayout<S: StyleLayerType, L> {
    layout: crate::layout::layout::NewLayout<L>,
    group: SmallVec<[Arc<EvaluatedLayer<S>>; 4]>,
    source_layer: SourceLayer,
}

impl<S: StyleLayerType, L: Layout<S>> ErasedLayout for TypedLayout<S, L> {
    fn has_dependencies(&self) -> bool {
        self.layout.has_dependencies
    }

    fn create_bucket(
        self: Box<Self>,
        resolved: Option<&Resolved>,
        built: &mut Built,
        build: &BuildParams,
    ) -> Result<(), Error> {
        let TypedLayout { layout, group, source_layer } = *self;
        let Some(bucket) = crate::layout::layout::build_layout(layout, &group, &source_layer, resolved, built, build)?
        else {
            return Ok(());
        };
        let layers = group.iter().map(|layer| -> (String, Arc<dyn ErasedEvaluatedLayer>) {
            (layer.style.id.clone(), Arc::<EvaluatedLayer<S>>::clone(layer))
        });
        built.buckets.add(Box::new(Stored::new(bucket)), layers);
        Ok(())
    }
}

impl<L: 'static> LayerEntry<L> {
    const INSTANCE: &'static Self = &LayerEntry(PhantomData);
}

/// The layer type `L` as a value.
pub(crate) const fn entry<L: Layer>() -> &'static dyn ErasedLayerType {
    LayerEntry::<L>::INSTANCE
}

/// The data a tile of `contents` holds for a layer of the style type `S`
/// laid out by `L` at `index` among the layout's layers; `None` where the
/// layout laid it out as a layer of another type.
pub(crate) fn laid<S: StyleLayerType, L: Layout<S>>(
    contents: &crate::tile::contents::GeometryContents,
    index: usize,
) -> Option<LayerBucket<'_, S, L::Bucket>> {
    let laid = contents.result.layer_render_data.laid(index)?;
    let layer: &dyn Any = laid.layer;
    Some(LayerBucket {
        bucket: laid.bucket.downcast_ref::<L::Bucket>()?,
        laid_with: layer.downcast_ref()?,
        id: laid.bucket.id(),
        contents,
    })
}

#[cfg(test)]
mod tests {
    use mln_style::evaluated::Evaluated;
    use mln_util::Color;

    use super::*;
    use crate::tests::fixture_layer::{Dot, DotPaintProperties};

    #[test]
    fn recover_keeps_the_allocation() {
        let paint = DotPaintProperties::PossiblyEvaluated {
            dot_color: Evaluated::Constant(Color::white()),
            dot_pattern: Evaluated::default(),
        };
        let style = Arc::new(LayerStyle::<Dot>::new("dot".to_owned(), "source".to_owned()));
        let crossfade = CrossfadeParameters { from_scale: 2.0, to_scale: 1.0, t: 0.25 };
        let layer: Arc<dyn ErasedEvaluatedLayer> =
            Arc::new(EvaluatedLayer { style, paint, crossfade, draws: false, entry: entry::<Dot>() });
        let recovered = EvaluatedLayer::<Dot>::recover(&layer).unwrap();
        assert!(std::ptr::addr_eq(Arc::as_ptr(&recovered), Arc::as_ptr(&layer)));
        assert_eq!(*recovered.paint.dot_color.uniform(), Color::white());
        assert_eq!(recovered.crossfade.t, 0.25);
    }
}
