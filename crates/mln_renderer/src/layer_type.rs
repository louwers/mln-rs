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
use mln_style::error::Error;
use mln_style::expression::Dependency;
use mln_style::layer::{ErasedLayerStyle, ErasedStyleLayerType, LayerStyle, LayerTypeEntry, PaintSet, StyleLayerType};
use mln_style::wire::SharedLayer;
use mln_vec_map::VecSet;
use mln_wire::{Reader, Wire, Writer, shared};
use smallvec::SmallVec;

use crate::bucket::Bucket;
use crate::layout::layout::{BuildParams, Layout, LayoutParameters, SourceLayer};
use crate::layout::tile_layout::{Built, Resolved};
use crate::render_layer::RenderLayer;
use crate::render_tile::TileView;

/// A layer type that the renderer draws.
pub(crate) trait LayerType: StyleLayerType {
    /// How a tile lays out a group of the type's layers.
    /// [`NoLayout`](crate::layout::layout::NoLayout) for a type that
    /// doesn't lay out.
    type Layout: Layout<Self>;

    /// The bucket the layout makes. [`NoBucket`](crate::bucket::NoBucket)
    /// for a type that doesn't lay out.
    type Bucket: Bucket + Wire;

    /// What a frame's tile shows a layer of the type, which its draws draw:
    /// its bucket in the tile's layout
    /// ([`LayerBucket`](crate::render_layer::LayerBucket)), a raster or
    /// raster-dem tile's own contents, or nothing for a type without a source
    /// ([`NoTileBucket`](crate::render_tile::NoTileBucket)).
    type TileBucket<'a>: TileView<'a>;

    /// Whether the layout of `group` can need glyphs or images. A tile
    /// makes such a group's layout before the dependencies are resolved,
    /// and every other group's once they are.
    fn layout_can_need_dependencies(_group: &[Arc<EvaluatedLayer<Self>>]) -> bool {
        false
    }

    /// The layout of `group`, layers of the same layout on `source_layer`,
    /// with the dependencies it records in `parameters`. `None` for a type
    /// that doesn't lay out.
    fn create_layout(
        _group: &[Arc<EvaluatedLayer<Self>>],
        _parameters: &mut LayoutParameters<'_>,
        _source_layer: SourceLayer,
    ) -> Result<Option<Self::Layout>, Error> {
        Ok(None)
    }

    /// How far outside a feature of `bucket` a query still hits it, from
    /// `layer`'s current values and the bucket's statistics of them.
    fn query_radius(_bucket: &Self::Bucket, _layer: &EvaluatedLayer<Self>) -> f32 {
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

    /// [`LayerType::layout_can_need_dependencies`]; false for a layer of
    /// another type.
    fn layout_can_need_dependencies(&self, group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool;

    /// [`LayerType::create_layout`]. Fails for a layer of another type.
    fn create_layout(
        &self,
        group: &[Arc<dyn ErasedEvaluatedLayer>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<Box<dyn ErasedLayout>>, Error>;

    /// [`LayerType::query_radius`]; 0 for a bucket or layer of another type.
    fn query_radius(&self, bucket: &dyn Bucket, layer: &Arc<dyn ErasedEvaluatedLayer>) -> f32;

    /// Reads the values that [`ErasedEvaluatedLayer::write`] wrote for a
    /// layer of the type.
    fn read_evaluated(&self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>>;

    /// Reads a bucket of the type, which [`Bucket::write_wire`] wrote.
    fn read_bucket(&self, r: &mut Reader<'_>) -> Option<Box<dyn Bucket>>;
}

impl<T: LayerType> ErasedLayerType for LayerTypeEntry<T> {
    fn create_render_layer(&self, style: &Arc<dyn ErasedLayerStyle>) -> Option<Box<dyn RenderLayer>> {
        LayerStyle::<T>::recover(style).map(T::render_layer)
    }

    fn layout_can_need_dependencies(&self, group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool {
        recover_group::<T>(group).is_ok_and(|group| T::layout_can_need_dependencies(&group))
    }

    fn create_layout(
        &self,
        group: &[Arc<dyn ErasedEvaluatedLayer>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<Box<dyn ErasedLayout>>, Error> {
        let layout = T::create_layout(&recover_group::<T>(group)?, parameters, source_layer)?;
        Ok(layout.map(|layout| -> Box<dyn ErasedLayout> { Box::new(TypedLayout::<T>(layout)) }))
    }

    fn query_radius(&self, bucket: &dyn Bucket, layer: &Arc<dyn ErasedEvaluatedLayer>) -> f32 {
        match (recover_bucket::<T>(bucket), EvaluatedLayer::<T>::recover(layer)) {
            (Some(bucket), Some(layer)) => T::query_radius(bucket, &layer),
            _ => 0.0,
        }
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

    fn read_bucket(&self, r: &mut Reader<'_>) -> Option<Box<dyn Bucket>> {
        Some(Box::new(T::Bucket::read(r)?))
    }
}

/// `bucket` as a bucket of a `T` layer, or `None` for another type's. The
/// engine holds buckets as `dyn Bucket`; a layer type gets its own through
/// this, in its glue, and never names another type's.
pub(crate) fn recover_bucket<T: LayerType>(bucket: &dyn Bucket) -> Option<&T::Bucket> {
    let any: &dyn Any = bucket;
    any.downcast_ref()
}

/// [`recover_bucket`], to change the bucket.
pub(crate) fn recover_bucket_mut<T: LayerType>(bucket: &mut dyn Bucket) -> Option<&mut T::Bucket> {
    let any: &mut dyn Any = bucket;
    any.downcast_mut()
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
    fn has_dependencies(&self) -> bool;

    /// Makes the bucket ([`Layout::create_bucket`]) and, if it has data,
    /// adds it to `built` for each of its layers that has none yet.
    fn create_bucket(
        &mut self,
        resolved: Option<&Resolved>,
        built: &mut Built,
        build: &BuildParams,
    ) -> Result<(), Error>;
}

struct TypedLayout<T: LayerType>(T::Layout);

impl<T: LayerType> ErasedLayout for TypedLayout<T> {
    fn has_dependencies(&self) -> bool {
        self.0.has_dependencies()
    }

    fn create_bucket(
        &mut self,
        resolved: Option<&Resolved>,
        built: &mut Built,
        build: &BuildParams,
    ) -> Result<(), Error> {
        let Some(bucket) = self.0.create_bucket(resolved, &mut built.feature_index, build)? else {
            return Ok(());
        };
        if !bucket.has_data() {
            return Ok(());
        }
        let layers = self.0.layers().iter().map(|(id, layer)| -> (String, Arc<dyn ErasedEvaluatedLayer>) {
            (id.clone(), Arc::<EvaluatedLayer<T>>::clone(layer))
        });
        built.buckets.add(Box::new(bucket), layers);
        Ok(())
    }
}

/// The layer type `T` as a value.
pub(crate) const fn entry<T: LayerType>() -> &'static dyn ErasedLayerType {
    LayerTypeEntry::<T>::INSTANCE
}

#[cfg(test)]
mod tests {
    use mln_map_types::MapMode;
    use mln_style::color::Color;
    use mln_style::layers::{Background, BackgroundPaintProperties, Circle, Symbol};
    use mln_style::property::evaluated::Evaluated;
    use mln_vec_map::VecMap;

    use super::*;
    use crate::buckets::circle_bucket::CircleBucket;

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
        assert!(!circles.layout_can_need_dependencies(std::slice::from_ref(&layer)));
        let bucket = CircleBucket::new(&VecMap::new(), MapMode::Static, 0.0);
        assert_eq!(circles.query_radius(&bucket, &layer), 0.0);
        let symbols = entry::<Symbol>();
        assert!(!symbols.layout_can_need_dependencies(&[layer]));
    }
}
