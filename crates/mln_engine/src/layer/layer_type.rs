//! The render half of the layer types: each marker type's [`LayerType`]
//! impl, which says how a tile lays out the type's layers and which render
//! layer draws them. The registry holds the types as
//! `&'static dyn ErasedLayerType` ([`entry`]).
//!
//! A layer's values for a frame are an [`EvaluatedLayer<T>`], shared as an
//! `Arc<dyn ErasedEvaluatedLayer>`. An erased value gets back to its type
//! only through [`EvaluatedLayer::recover`] or [`evaluated`].

use std::any::Any;
use std::sync::Arc;

use mln_style::CrossfadeParameters;
use mln_style::collections::VecSet;
use mln_style::error::Error;
use mln_style::expression::Dependency;
use mln_style::layer::{ErasedLayerStyle, ErasedStyleLayerType, LayerStyle, LayerTypeEntry, PaintSet, StyleLayerType};
use mln_style::wire::SharedLayer;
use mln_wire::{Reader, Wire, Writer, shared};
use smallvec::SmallVec;

use crate::layer::bucket::{ErasedBucket, Stored};
use crate::layer::bucket_parameters::BucketParameters;
use crate::layer::layout::{BuildCx, Layout, LayoutCx, LayoutParameters, SourceLayer, layers_by_id};
use crate::layer::render_layer::RenderLayer;
use crate::layer::tile_input::input::Find;
use crate::layer::tile_input::{TileData, TileInput};
use crate::layer::tile_layout::{Built, Resolved};
use crate::tile_store::contents::GeometryContents;

/// A layer type that the renderer draws.
pub(crate) trait LayerType: StyleLayerType {
    /// What the type's layers draw from: the features of a geometry
    /// source's tiles, which a layout lays out into a bucket per tile
    /// ([`Features`](crate::layer::tile_input::Features)), a raster or
    /// raster-dem source's tiles, or none, for a type without a source. The
    /// registry checks that it is of the kind of source that the type's
    /// style half names.
    type Input: TileInput<Self>;

    /// How far outside a feature of `data`, a tile's bucket of the layer, a
    /// query still hits it, from `layer`'s current values and the bucket's
    /// statistics of them. The engine asks only a type whose input lays
    /// out.
    fn query_radius(_data: TileData<'_, Self>, _layer: &EvaluatedLayer<Self>) -> f32 {
        0.0
    }

    /// Whether the type has cross-faded properties, whose evaluation takes
    /// the frame's cross-fade.
    const CROSSFADES: bool = false;

    fn render_layer(style: Arc<LayerStyle<Self>>) -> Box<dyn RenderLayer>;
}

/// The values of a layer of the type `T` for a frame.
pub(crate) struct EvaluatedLayer<T: LayerType> {
    pub(crate) style: Arc<LayerStyle<T>>,
    pub(crate) paint: <T::PaintProperties as PaintSet>::Evaluated,
    /// The default for a type without cross-faded properties.
    pub(crate) crossfade: CrossfadeParameters,
    /// The `RenderPass` bits the layer draws in.
    pub(crate) render_passes: u8,
}

impl<T: LayerType> EvaluatedLayer<T> {
    /// `layer` as a `T` layer's values, the same allocation, or `None` for
    /// another type's.
    pub(crate) fn recover(layer: &Arc<dyn ErasedEvaluatedLayer>) -> Option<Arc<Self>> {
        let any: Arc<dyn Any + Send + Sync> = Arc::<dyn ErasedEvaluatedLayer>::clone(layer);
        any.downcast().ok()
    }

    pub(crate) fn new(style: Arc<LayerStyle<T>>) -> Self {
        EvaluatedLayer::with_paint(style, Default::default())
    }

    /// The values of the layer `style` with its paint properties evaluated
    /// to `paint`, without a cross-fade or render passes.
    pub(crate) fn with_paint(style: Arc<LayerStyle<T>>, paint: <T::PaintProperties as PaintSet>::Evaluated) -> Self {
        EvaluatedLayer { style, paint, crossfade: CrossfadeParameters::default(), render_passes: 0 }
    }
}

impl<T: LayerType> std::fmt::Debug for EvaluatedLayer<T> {
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

impl<T: LayerType> ErasedEvaluatedLayer for EvaluatedLayer<T> {
    fn entry(&self) -> &'static dyn ErasedLayerType {
        entry::<T>()
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
pub(crate) trait ErasedLayerType: ErasedStyleLayerType {
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

    /// [`LayerType::query_radius`] of the bucket that `contents` laid out
    /// for `layer`; 0 where they laid out none of the type's.
    fn query_radius(&self, contents: &GeometryContents, layer: &Arc<dyn ErasedEvaluatedLayer>) -> f32;

    /// Reads the values that [`ErasedEvaluatedLayer::write`] wrote for a
    /// layer of the type.
    fn read_evaluated(&self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>>;

    /// Reads a bucket of the type, which [`ErasedBucket::write_wire`] wrote.
    fn read_bucket(&self, r: &mut Reader<'_>) -> Option<Box<dyn ErasedBucket>>;
}

impl<T: LayerType> ErasedLayerType for LayerTypeEntry<T> {
    fn create_render_layer(&self, style: &Arc<dyn ErasedLayerStyle>) -> Option<Box<dyn RenderLayer>> {
        LayerStyle::<T>::recover(style).map(T::render_layer)
    }

    fn needs_resources(&self, group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool {
        T::Input::needs_resources(group)
    }

    fn create_layout(
        &self,
        group: &[Arc<dyn ErasedEvaluatedLayer>],
        parameters: LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<Box<dyn ErasedLayout>>, Error> {
        T::Input::create_layout(group, parameters, source_layer)
    }

    fn query_radius(&self, contents: &GeometryContents, layer: &Arc<dyn ErasedEvaluatedLayer>) -> f32 {
        let Some(layer) = EvaluatedLayer::<T>::recover(layer) else {
            return 0.0;
        };
        T::Input::laid(contents, &layer.style.id).map_or(0.0, |data| T::query_radius(data, &layer))
    }

    fn read_evaluated(&self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>> {
        let style = shared::read::<SharedLayer>(r)?;
        let Some(style) = LayerStyle::<T>::recover(&style) else {
            return r.fail("the evaluated values of a layer of another type");
        };
        Some(Arc::new(EvaluatedLayer::<T> {
            style,
            paint: Wire::read(r)?,
            crossfade: Wire::read(r)?,
            render_passes: Wire::read(r)?,
        }))
    }

    fn read_bucket(&self, r: &mut Reader<'_>) -> Option<Box<dyn ErasedBucket>> {
        T::Input::read_bucket(r)
    }
}

/// [`Layout::needs_resources`] of `L`, a layout of the type `T`, for
/// `group`; false if a layer of it is another type's.
pub(crate) fn needs_resources<T: LayerType, L: Layout<T>>(group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool {
    recover_group::<T>(group).is_ok_and(|group| L::needs_resources(&group))
}

/// The layout `L` of `group`, layers of the type `T` of the same layout on
/// `source_layer`, with the dependencies it records in `parameters`. Fails
/// for a layer of another type.
pub(crate) fn create_layout<T: LayerType, L: Layout<T>>(
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
fn recover_group<T: LayerType>(
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
struct TypedLayout<T: LayerType, L> {
    layout: L,
    layers: SmallVec<[Arc<EvaluatedLayer<T>>; 4]>,
    source_layer: SourceLayer,
    needs_resources: bool,
}

impl<T: LayerType, L: Layout<T>> ErasedLayout for TypedLayout<T, L> {
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

/// The layer type `T` as a value.
pub(crate) const fn entry<T: LayerType>() -> &'static dyn ErasedLayerType {
    LayerTypeEntry::<T>::INSTANCE
}

#[cfg(test)]
mod tests {
    use mln_style::color::Color;
    use mln_style::layers::{Background, BackgroundPaintProperties, Circle, Symbol};
    use mln_style::property::evaluated::Evaluated;

    use super::*;

    fn background() -> Arc<dyn ErasedEvaluatedLayer> {
        let paint = BackgroundPaintProperties::PossiblyEvaluated {
            background_color: Evaluated::Constant(Color::new(0.25, 0.5, 0.75, 1.0)),
            background_opacity: Evaluated::Constant(0.5),
            ..BackgroundPaintProperties::PossiblyEvaluated::default()
        };
        let style = Arc::new(LayerStyle::<Background>::new("background".to_owned(), String::new()));
        let crossfade = CrossfadeParameters { from_scale: 2.0, to_scale: 1.0, t: 0.25 };
        Arc::new(EvaluatedLayer { style, paint, crossfade, render_passes: 0 })
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
        let circles = entry::<Circle>();
        assert!(!circles.needs_resources(std::slice::from_ref(&layer)));
        let symbols = entry::<Symbol>();
        assert!(!symbols.needs_resources(&[layer]));
    }
}
