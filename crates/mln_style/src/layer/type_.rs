//! Layer types as values.
//!
//! A layer type is a marker type that implements [`StyleLayerType`]. Each
//! layer of the type holds a [`LayerStyle<T>`], shared as an
//! `Arc<dyn ErasedLayerStyle>`, and a [`StyleRegistry`](crate::StyleRegistry)
//! holds the types as `&'static dyn ErasedStyleLayerType`, by name. An erased
//! value gets back to its type only through [`LayerStyle::recover`] and
//! [`LayerStyle::recover_ref`].

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use mln_wire::{Reader, Wire, Writer};

use crate::collections::VecSet;
use crate::conversion::Convertible;
use crate::expression::{Dependency, Expr, Functions};
use crate::feature::ValueObject;
use crate::filter::Filter;
use crate::layer::FontStack;
use crate::member::Members;
use crate::property::evaluation::PropertyEvaluationParameters;
use crate::property::table::{
    self, OwnerSlot, Properties, PropertyTable, PropertyVisitor, SetPropertyError, StyleProperty,
};
use crate::property::transition::TransitionParameters;
use crate::property::value::PropertyValue;
use crate::types::{LayerBlendType, VisibilityType};
use crate::wire::optional_expression;

/// Where a layer type's layers get their data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// No source: the layer draws without one.
    None,
    /// Vector or GeoJSON tiles, laid out into buckets.
    Geometry,
    Raster,
    RasterDem,
}

/// What the engine needs to know about a layer type besides its properties.
/// The name identifies the type: two registries that agree on a name agree
/// on the type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerTypeInfo {
    /// The layer type as style JSON names it.
    pub name: &'static str,
    pub source: SourceKind,
    /// Whether style JSON can create a layer of the type.
    pub from_json: bool,
}

/// A set of layout properties as the style sets them, or [`NoProperties`].
pub trait LayoutSet: Clone + Default + PartialEq + std::fmt::Debug + Wire + Send + Sync + 'static {
    /// The combined dependencies of the expressions.
    fn dependencies(&self) -> Dependency;

    fn collect_global_state_refs(&self, refs: &mut VecSet<String>);
}

/// A set of paint properties as the style sets them, or [`NoProperties`].
pub trait PaintSet: Clone + Default + PartialEq + std::fmt::Debug + Wire + Send + Sync + 'static {
    /// The set evaluated for a frame: a value per property, which for a
    /// data-driven one can still differ by feature.
    type Evaluated: Clone + Default + std::fmt::Debug + Wire + Send + Sync + 'static;

    /// The set's values in transition from earlier values, which evaluate
    /// to [`Evaluated`](Self::Evaluated) for a frame.
    type Unevaluated: Clone + Default + std::fmt::Debug + Send + Sync + 'static;

    /// The set's values, with nothing to transition from.
    fn untransitioned(&self) -> Self::Unevaluated;

    /// The set's values, in transition from `prior`.
    fn transitioned(&self, parameters: &TransitionParameters, prior: Self::Unevaluated) -> Self::Unevaluated;

    /// Evaluates `unevaluated` for a frame, copying from `previous`, the
    /// last frame's, each value that can't have changed.
    fn evaluate_with_previous(
        unevaluated: &mut Self::Unevaluated,
        parameters: &PropertyEvaluationParameters,
        previous: &Self::Evaluated,
    ) -> Self::Evaluated;

    /// Whether a value of `unevaluated` is still in transition.
    fn has_transition(unevaluated: &Self::Unevaluated) -> bool;

    /// The combined dependencies of the values of `unevaluated`.
    fn dependencies(unevaluated: &Self::Unevaluated) -> Dependency;

    /// The combined dependencies of the values in `evaluated` that still
    /// differ by feature.
    fn evaluated_dependencies(evaluated: &Self::Evaluated) -> Dependency;

    /// Bit i is set when the i-th data-driven property of `evaluated` is
    /// the same for every feature.
    fn constants_mask(evaluated: &Self::Evaluated) -> u64;

    /// Adds the global-state keys that the values in `evaluated` still read
    /// to `refs`.
    fn collect_evaluated_global_state_refs(evaluated: &Self::Evaluated, refs: &mut VecSet<String>);
}

/// The property set of a layer type that has no properties of a scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoProperties;

impl Wire for NoProperties {
    fn write(&self, _w: &mut Writer) {}

    fn read(_r: &mut Reader<'_>) -> Option<Self> {
        Some(NoProperties)
    }
}

impl LayoutSet for NoProperties {
    fn dependencies(&self) -> Dependency {
        Dependency::NONE
    }

    fn collect_global_state_refs(&self, _refs: &mut VecSet<String>) {}
}

impl PaintSet for NoProperties {
    type Evaluated = NoProperties;
    type Unevaluated = NoProperties;

    fn untransitioned(&self) -> NoProperties {
        NoProperties
    }

    fn transitioned(&self, _parameters: &TransitionParameters, _prior: NoProperties) -> NoProperties {
        NoProperties
    }

    fn evaluate_with_previous(
        _unevaluated: &mut NoProperties,
        _parameters: &PropertyEvaluationParameters,
        _previous: &NoProperties,
    ) -> NoProperties {
        NoProperties
    }

    fn has_transition(_unevaluated: &NoProperties) -> bool {
        false
    }

    fn dependencies(_unevaluated: &NoProperties) -> Dependency {
        Dependency::NONE
    }

    fn evaluated_dependencies(_evaluated: &NoProperties) -> Dependency {
        Dependency::NONE
    }

    fn constants_mask(_evaluated: &NoProperties) -> u64 {
        0
    }

    fn collect_evaluated_global_state_refs(_evaluated: &NoProperties, _refs: &mut VecSet<String>) {}
}

/// A layer type: what its layers hold and how they differ.
pub trait StyleLayerType: Sized + Send + Sync + 'static {
    const INFO: LayerTypeInfo;

    /// The type's property table, generated by //tools/style-codegen as
    /// `PROPERTIES` in `<type>_layer_properties.rs`.
    const PROPERTIES: &'static PropertyTable;

    type LayoutProperties: LayoutSet;
    type PaintProperties: PaintSet;

    /// Data of a layer that isn't a property.
    type Extra: Clone + Default + Send + Sync + 'static;

    /// Calls `visitor` with the field of the property in `slot` of
    /// [`PROPERTIES`](Self::PROPERTIES).
    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output>;

    fn populate_font_stack(_style: &LayerStyle<Self>, _out: &mut VecSet<FontStack>) {}

    /// Whether a section of a formatted value of `style` overrides the
    /// `overridable` paint properties.
    fn has_section_overrides(_style: &LayerStyle<Self>) -> bool {
        false
    }

    /// The layout property that raises the layer's content above the
    /// ground, if the type has one.
    fn content_elevation(_style: &LayerStyle<Self>) -> Option<&PropertyValue<f32>> {
        None
    }

    /// How a layer of the type with the paint values `paint` composites
    /// onto what is drawn beneath it: its `*-layer-blend`, or `normal` for a
    /// type without one.
    fn layer_blend(_paint: &<Self::PaintProperties as PaintSet>::Evaluated) -> LayerBlendType {
        LayerBlendType::Normal
    }
}

/// The members every layer has.
#[derive(Clone, Debug, mln_wire::Wire)]
pub struct LayerBase {
    // Backs an expression-valued "visibility" layout value; the evaluated
    // result is kept in `visibility` and re-evaluated by the style whenever
    // the global state changes.
    #[wire(with = "optional_expression")]
    pub(crate) visibility_expression: Option<Arc<Expr>>,

    pub id: String,
    pub source: String,
    pub source_layer: String,
    pub filter: Filter,
    pub min_zoom: f32,
    pub max_zoom: f32,
    pub visibility: VisibilityType,
    /// The layer's extension members, which tile workers don't get.
    #[wire(skip)]
    pub(crate) members: Members,
}

impl LayerBase {
    #[must_use]
    pub fn new(layer_id: String, source_id: String) -> Self {
        LayerBase {
            visibility_expression: None,
            id: layer_id,
            source: source_id,
            source_layer: String::new(),
            filter: Filter::default(),
            min_zoom: f32::NEG_INFINITY,
            max_zoom: f32::INFINITY,
            visibility: VisibilityType::Visible,
            members: Members::default(),
        }
    }

    /// The layer's extension members.
    #[must_use]
    pub fn members(&self) -> &Members {
        &self.members
    }

    pub(crate) fn get_filter_dependencies(&self) -> Dependency {
        match &self.filter.expression {
            Some(expression) => expression.dependencies(),
            None => Dependency::NONE,
        }
    }

    pub(crate) fn collect_filter_global_state_refs(&self, refs: &mut VecSet<String>) {
        if let Some(expression) = &self.filter.expression {
            crate::expression::collect_global_state_refs(expression.as_ref(), refs);
        }
    }
}

/// The style data of a layer of the type `T`: the members every layer has,
/// and its type's properties.
#[derive(mln_wire::Wire)]
#[wire(bound = "")]
pub struct LayerStyle<T: StyleLayerType> {
    pub(crate) base: LayerBase,
    pub layout: T::LayoutProperties,
    pub paint: T::PaintProperties,
    /// The type's data that isn't a property, which tile workers don't get.
    #[wire(skip)]
    pub extra: T::Extra,
}

// Written out: `derive(Clone)` would require `T: Clone` of the marker type.
impl<T: StyleLayerType> Clone for LayerStyle<T> {
    fn clone(&self) -> Self {
        LayerStyle {
            base: self.base.clone(),
            layout: self.layout.clone(),
            paint: self.paint.clone(),
            extra: self.extra.clone(),
        }
    }
}

impl<T: StyleLayerType> std::fmt::Debug for LayerStyle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerStyle")
            .field("type", &T::INFO.name)
            .field("base", &self.base)
            .field("layout", &self.layout)
            .field("paint", &self.paint)
            .finish_non_exhaustive()
    }
}

impl<T: StyleLayerType> LayerStyle<T> {
    /// A layer named `layer_id` of the source `source_id`, with every
    /// property undefined.
    #[must_use]
    pub fn new(layer_id: String, source_id: String) -> Self {
        LayerStyle {
            base: LayerBase::new(layer_id, source_id),
            layout: T::LayoutProperties::default(),
            paint: T::PaintProperties::default(),
            extra: T::Extra::default(),
        }
    }

    /// `style` as a `T` layer's, or `None` for another type's. The result
    /// shares `style`'s allocation.
    #[must_use]
    pub fn recover(style: &Arc<dyn ErasedLayerStyle>) -> Option<Arc<Self>> {
        let any: Arc<dyn Any + Send + Sync> = Arc::<dyn ErasedLayerStyle>::clone(style);
        any.downcast().ok()
    }

    /// `style` as a `T` layer's, or `None` for another type's.
    #[must_use]
    pub fn recover_ref(style: &dyn ErasedLayerStyle) -> Option<&Self> {
        let any: &dyn Any = style;
        any.downcast_ref()
    }
}

impl<T: StyleLayerType> std::ops::Deref for LayerStyle<T> {
    type Target = LayerBase;
    fn deref(&self) -> &LayerBase {
        &self.base
    }
}

impl<T: StyleLayerType> std::ops::DerefMut for LayerStyle<T> {
    fn deref_mut(&mut self) -> &mut LayerBase {
        &mut self.base
    }
}

impl<T: StyleLayerType> Properties for LayerStyle<T> {
    const TABLE: &'static PropertyTable = T::PROPERTIES;

    fn visit<V: PropertyVisitor<Self>>(slot: usize, visitor: V) -> Option<V::Output> {
        T::visit_property(slot, visitor)
    }
}

/// A layer's style data, whichever its type.
pub trait ErasedLayerStyle: Any + Send + Sync {
    fn base(&self) -> &LayerBase;

    fn info(&self) -> LayerTypeInfo;

    /// Whether going from `self` to `other` changes what the layer's tiles
    /// lay out. True when `other` is another type's.
    fn layout_difference(&self, other: &dyn ErasedLayerStyle) -> bool;

    /// The dependencies of what affects tile layout: the filter and the
    /// layout properties.
    fn layout_dependencies(&self) -> Dependency;

    /// Adds the global-state keys read by what affects tile layout, as
    /// [`layout_dependencies`](Self::layout_dependencies) counts it,
    /// to `refs`.
    fn collect_layout_global_state_refs(&self, refs: &mut VecSet<String>);

    fn populate_font_stack(&self, out: &mut VecSet<FontStack>);

    fn content_elevation(&self) -> Option<&PropertyValue<f32>>;

    fn layer_type(&self) -> &'static dyn ErasedStyleLayerType;

    /// The property `name` (or a paint property's `<name>-transition`),
    /// undefined for a name the type lacks.
    fn get_property(&self, name: &str) -> StyleProperty;

    /// Writes the defined properties into a layer's style JSON `out`, under
    /// "paint" and "layout". A key already there stays.
    fn serialize_properties(&self, out: &mut ValueObject);

    /// A copy named `id`, without the paint properties.
    fn copy_without_paint(&self, id: &str) -> Arc<dyn ErasedLayerStyle>;

    /// Writes the value for its type's [`ErasedStyleLayerType::read_style`].
    fn write(&self, w: &mut Writer);
}

impl<T: StyleLayerType> ErasedLayerStyle for LayerStyle<T> {
    fn base(&self) -> &LayerBase {
        &self.base
    }

    fn info(&self) -> LayerTypeInfo {
        T::INFO
    }

    fn layout_difference(&self, other: &dyn ErasedLayerStyle) -> bool {
        LayerStyle::<T>::recover_ref(other).is_none_or(|other| layout_difference(self, other))
    }

    fn layout_dependencies(&self) -> Dependency {
        self.layout.dependencies() | self.base.get_filter_dependencies()
    }

    fn collect_layout_global_state_refs(&self, refs: &mut VecSet<String>) {
        self.layout.collect_global_state_refs(refs);
        self.base.collect_filter_global_state_refs(refs);
    }

    fn populate_font_stack(&self, out: &mut VecSet<FontStack>) {
        T::populate_font_stack(self, out);
    }

    fn content_elevation(&self) -> Option<&PropertyValue<f32>> {
        T::content_elevation(self)
    }

    fn layer_type(&self) -> &'static dyn ErasedStyleLayerType {
        style_entry::<T>()
    }

    fn get_property(&self, name: &str) -> StyleProperty {
        table::get_property(self, name)
    }

    fn serialize_properties(&self, out: &mut ValueObject) {
        table::serialize_properties(self, out);
    }

    fn copy_without_paint(&self, id: &str) -> Arc<dyn ErasedLayerStyle> {
        let mut copy = self.clone();
        copy.base.id = id.to_owned();
        copy.paint = T::PaintProperties::default();
        Arc::new(copy)
    }

    fn write(&self, w: &mut Writer) {
        Wire::write(self, w);
    }
}

fn layout_difference<T: StyleLayerType>(a: &LayerStyle<T>, b: &LayerStyle<T>) -> bool {
    T::INFO.source == SourceKind::Geometry
        && (a.base.filter != b.base.filter
            || a.base.visibility != b.base.visibility
            || table::layout_properties_differ(a, b, T::has_section_overrides(a)))
}

/// A layer type, whichever it is.
pub trait ErasedStyleLayerType: Send + Sync + 'static {
    fn info(&self) -> LayerTypeInfo;

    /// The style data of a layer of the type named `id` of the source
    /// `source`, with every property undefined.
    fn create(&self, id: &str, source: &str) -> Arc<dyn ErasedLayerStyle>;

    /// Sets the property `name` (or a paint property's `<name>-transition`)
    /// of `style`, which must be of the type, from a style JSON value whose
    /// expressions parse against `functions`. `style` is copied only when
    /// the value changes it and it is shared.
    ///
    /// # Errors
    ///
    /// When the type has no such property, or the value doesn't convert.
    fn set_property(
        &self,
        style: &mut Arc<dyn ErasedLayerStyle>,
        name: &str,
        value: &Convertible<'_>,
        functions: &Arc<Functions>,
    ) -> Result<(), SetPropertyError>;

    /// Calls `f` with the members every layer has of `style`, which must be
    /// of the type, copying it first if it is shared.
    fn mutate_base(&self, style: &mut Arc<dyn ErasedLayerStyle>, f: &mut dyn FnMut(&mut LayerBase));

    /// Reads the value that [`ErasedLayerStyle::write`] wrote for a layer of
    /// the type.
    fn read_style(&self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedLayerStyle>>;
}

/// The [`ErasedStyleLayerType`] of `T`, which [`style_entry`] returns.
pub struct LayerTypeEntry<T>(PhantomData<fn() -> T>);

impl<T: 'static> LayerTypeEntry<T> {
    pub const INSTANCE: &'static Self = &LayerTypeEntry(PhantomData);
}

impl<T: StyleLayerType> ErasedStyleLayerType for LayerTypeEntry<T> {
    fn info(&self) -> LayerTypeInfo {
        T::INFO
    }

    fn create(&self, id: &str, source: &str) -> Arc<dyn ErasedLayerStyle> {
        Arc::new(LayerStyle::<T>::new(id.to_owned(), source.to_owned()))
    }

    fn set_property(
        &self,
        style: &mut Arc<dyn ErasedLayerStyle>,
        name: &str,
        value: &Convertible<'_>,
        functions: &Arc<Functions>,
    ) -> Result<(), SetPropertyError> {
        let mut slot = ErasedSlot::<T>(style, PhantomData);
        if slot.get().is_none() {
            return Err(SetPropertyError::Unknown);
        }
        table::set_property(&mut slot, name, value, functions)
    }

    fn mutate_base(&self, style: &mut Arc<dyn ErasedLayerStyle>, f: &mut dyn FnMut(&mut LayerBase)) {
        if let Some(style) = ErasedSlot::<T>(style, PhantomData).make_mut() {
            f(&mut style.base);
        }
    }

    fn read_style(&self, r: &mut Reader<'_>) -> Option<Arc<dyn ErasedLayerStyle>> {
        Some(Arc::new(LayerStyle::<T>::read(r)?))
    }
}

/// An erased style that is a `LayerStyle<T>`, as the slot that property
/// changes copy and change it in.
struct ErasedSlot<'a, T>(&'a mut Arc<dyn ErasedLayerStyle>, PhantomData<fn() -> T>);

impl<T: StyleLayerType> OwnerSlot<LayerStyle<T>> for ErasedSlot<'_, T> {
    fn get(&self) -> Option<&LayerStyle<T>> {
        LayerStyle::recover_ref(&**self.0)
    }

    #[inline(never)]
    fn make_mut(&mut self) -> Option<&mut LayerStyle<T>> {
        if Arc::get_mut(self.0).is_none() {
            let copy = self.get()?.clone();
            *self.0 = Arc::new(copy);
        }
        let style: &mut dyn ErasedLayerStyle = Arc::get_mut(self.0)?;
        let any: &mut dyn Any = style;
        any.downcast_mut()
    }
}

/// The layer type `T` as a value.
#[must_use]
pub const fn style_entry<T: StyleLayerType>() -> &'static dyn ErasedStyleLayerType {
    LayerTypeEntry::<T>::INSTANCE
}

// Debug is not a supertrait of this trait, nor of the other layer and source
// traits that have a `dyn` Debug impl like this one: through the vtable it
// would keep every implementation's derived Debug (and those of everything it
// holds) in the binary. Each prints what tells its values apart instead.
impl std::fmt::Debug for dyn ErasedLayerStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerStyle")
            .field("id", &self.base().id)
            .field("type", &self.info().name)
            .finish_non_exhaustive()
    }
}
