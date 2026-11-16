//! The render half of the layer types.
//!
//! A layer type's render half is its marker type's [`LayerType`] impl, beside
//! the type's render layer, which it makes from the layer's own style data.
//! The registry holds the types as `&'static dyn ErasedLayerType`
//! ([`entry`]), the render half of the same `LayerTypeEntry<T>` value that a
//! style registry holds as the style half.
//!
//! A layer's values for a frame are an [`EvaluatedLayer<T>`], which the render
//! layer, its tiles' render data and the tile workers share as an
//! `Arc<dyn ErasedEvaluatedLayer>`. An erased value gets back to its type
//! only through [`evaluated`], which checks the value's type.

use std::any::Any;
use std::sync::Arc;

use mln_style::CrossfadeParameters;
use mln_style::expression::Dependency;
use mln_style::layer_type::{
    ErasedLayerStyle, ErasedStyleLayerType, LayerStyle, LayerTypeEntry, PaintSet, StyleLayerType,
};
use mln_style::serialization::shared_layer;
use mln_vec_map::VecSet;
use mln_wire::{Reader, Wire, Writer};

use crate::render_layer::RenderLayerPtr;

/// A layer type that the renderer draws.
pub(crate) trait LayerType: StyleLayerType {
    /// The render layer that draws the layer `style`.
    fn render_layer(style: Arc<LayerStyle<Self>>) -> RenderLayerPtr;
}

/// The values of a layer of the type `T` for a frame: its style data, its
/// paint properties evaluated, the cross-fade of its patterns and the
/// render passes it draws in.
pub(crate) struct EvaluatedLayer<T: LayerType> {
    pub(crate) style: Arc<LayerStyle<T>>,
    pub(crate) paint: <T::PaintProperties as PaintSet>::Evaluated,
    /// The default for a type without cross-faded properties.
    pub(crate) crossfade: CrossfadeParameters,
    /// The `RenderPass` bits the layer draws in.
    pub(crate) render_passes: u8,
}

impl<T: LayerType> EvaluatedLayer<T> {
    /// The values of the layer `style` before its first evaluation.
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
    /// The layer's style data.
    fn style(&self) -> &dyn ErasedLayerStyle;

    /// The layer's style data, shared.
    fn shared_style(&self) -> Arc<dyn ErasedLayerStyle>;

    /// The `RenderPass` bits the layer draws in.
    fn render_passes(&self) -> u8;

    /// Bit i is set when the i-th data-driven paint property is the same for
    /// every feature.
    fn constants_mask(&self) -> u64;

    /// The combined dependencies of the evaluated paint properties that still
    /// differ by feature.
    fn evaluated_dependencies(&self) -> Dependency;

    /// Adds the global-state keys that the evaluated paint properties still
    /// read to `refs`.
    fn collect_evaluated_global_state_refs(&self, refs: &mut VecSet<String>);

    /// Writes the values for their type's [`ErasedLayerType::read_evaluated`]:
    /// the style data by key, then the rest.
    fn write(&self, w: &mut Writer);
}

impl<T: LayerType> ErasedEvaluatedLayer for EvaluatedLayer<T> {
    fn style(&self) -> &dyn ErasedLayerStyle {
        &*self.style
    }

    fn shared_style(&self) -> Arc<dyn ErasedLayerStyle> {
        Arc::<LayerStyle<T>>::clone(&self.style)
    }

    fn render_passes(&self) -> u8 {
        self.render_passes
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
        shared_layer::write(&self.shared_style(), w);
        self.paint.write(w);
        self.crossfade.write(w);
        self.render_passes.write(w);
    }
}

// Debug is not a supertrait, for the reason at mln_style's
// `impl Debug for dyn Impl`.
impl std::fmt::Debug for dyn ErasedEvaluatedLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvaluatedLayer")
            .field("id", &self.style().id)
            .field("type", &self.style().get_type_info().name)
            .finish_non_exhaustive()
    }
}

/// `layer` as a `T` layer's values.
///
/// # Panics
///
/// If `layer` is another type's.
pub(crate) fn evaluated<T: LayerType>(layer: &Arc<dyn ErasedEvaluatedLayer>) -> &EvaluatedLayer<T> {
    let any: &dyn Any = &**layer;
    match any.downcast_ref() {
        Some(layer) => layer,
        None => panic!(
            "the evaluated values of a {} layer, not a {} layer",
            layer.style().get_type_info().name,
            T::INFO.name
        ),
    }
}

/// A layer type that the renderer draws, whichever it is.
pub(crate) trait ErasedLayerType: ErasedStyleLayerType {
    /// The render layer that draws the layer `style`, or `None` if `style`
    /// is another type's.
    fn create_render_layer(&self, style: &Arc<dyn ErasedLayerStyle>) -> Option<RenderLayerPtr>;

    /// Reads the values that [`ErasedEvaluatedLayer::write`] wrote for a
    /// layer of the type.
    fn read_evaluated(&self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>>;
}

impl<T: LayerType> ErasedLayerType for LayerTypeEntry<T> {
    fn create_render_layer(&self, style: &Arc<dyn ErasedLayerStyle>) -> Option<RenderLayerPtr> {
        LayerStyle::<T>::recover(style).map(T::render_layer)
    }

    fn read_evaluated(&self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>> {
        let style = shared_layer::read(r)?;
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
}

/// The layer type `T` as a value.
pub(crate) const fn entry<T: LayerType>() -> &'static dyn ErasedLayerType {
    LayerTypeEntry::<T>::INSTANCE
}

#[cfg(test)]
mod tests {
    use mln_style::evaluated::Evaluated;
    use mln_style::layers::{Background, BackgroundPaintProperties, Circle};
    use mln_util::Color;

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
    fn evaluated_gets_the_values_of_their_type() {
        let layer = background();
        let layer = evaluated::<Background>(&layer);
        assert_eq!(*layer.paint.background_color.uniform(), Color::new(0.25, 0.5, 0.75, 1.0));
        assert_eq!(*layer.paint.background_opacity.uniform(), 0.5);
        assert_eq!(layer.crossfade.t, 0.25);
    }

    #[test]
    #[should_panic(expected = "background layer, not a circle layer")]
    fn evaluated_panics_for_another_type() {
        let layer = background();
        let _ = evaluated::<Circle>(&layer);
    }
}
