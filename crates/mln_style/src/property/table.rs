//! The properties of a layer type, the light or the sky, as one table each,
//! and the generic access by style-spec name that the style JSON, the runtime
//! and the C API use.
//!
//! A table lists each property's name, whether it is paint or layout, and how
//! its value converts. A property's slot is its index in the table. The type
//! that holds the values implements `Properties`, which maps a slot to the
//! field that holds it, typed, so that reading, converting and comparing a
//! value needs no downcast.

use std::sync::Arc;

use crate::conversion::to_value::make_transition_style_property;
use crate::conversion::{Converter, Convertible, Error};
use crate::expression::Functions;
use crate::feature::{Value, ValueObject};
use crate::layer::PropertyScope;
use crate::property::set::Transitionable;
use crate::property::transition::TransitionOptions;
use crate::property::value::StyleValue;

/// One property in a [`PropertyTable`].
#[derive(Debug)]
pub struct PropertyInfo {
    /// The style-spec name.
    pub(crate) name: &'static str,
    pub(crate) scope: PropertyScope,
    /// Whether the value may be an expression that reads the feature.
    pub(crate) data_driven: bool,
    /// Whether a string constant's `{token}`s become an expression.
    pub(crate) tokens: bool,
    /// Whether a section of a formatted value can override the paint
    /// property's value.
    pub(crate) overridable: bool,
    /// Whether the property is `cross-faded-data-driven` in the spec: its
    /// values (patterns and dash arrays) are dependencies of the layout, so
    /// any change to one lays the layer out again.
    pub(crate) cross_faded: bool,
}

impl PropertyInfo {
    #[must_use]
    pub(crate) const fn paint(name: &'static str) -> Self {
        PropertyInfo {
            name,
            scope: PropertyScope::Paint,
            data_driven: false,
            tokens: false,
            overridable: false,
            cross_faded: false,
        }
    }

    #[must_use]
    pub(crate) const fn layout(name: &'static str) -> Self {
        PropertyInfo {
            name,
            scope: PropertyScope::Layout,
            data_driven: false,
            tokens: false,
            overridable: false,
            cross_faded: false,
        }
    }

    #[must_use]
    pub(crate) const fn data_driven(self) -> Self {
        PropertyInfo { data_driven: true, ..self }
    }

    #[must_use]
    pub(crate) const fn tokens(self) -> Self {
        PropertyInfo { tokens: true, ..self }
    }

    #[must_use]
    pub(crate) const fn overridable(self) -> Self {
        PropertyInfo { overridable: true, ..self }
    }

    #[must_use]
    pub(crate) const fn cross_faded(self) -> Self {
        PropertyInfo { cross_faded: true, ..self }
    }
}

/// The properties of one layer type, the light or the sky, in the style
/// spec's order: paint properties, then layout properties.
#[derive(Debug)]
pub struct PropertyTable {
    properties: &'static [PropertyInfo],
    /// The slots, sorted by property name.
    by_name: &'static [u8],
}

impl PropertyTable {
    /// `by_name` lists the slots of `properties` sorted by name.
    #[must_use]
    pub const fn new(properties: &'static [PropertyInfo], by_name: &'static [u8]) -> Self {
        PropertyTable { properties, by_name }
    }

    /// The slot of the property named `name`.
    #[must_use]
    pub(crate) fn find(&self, name: &str) -> Option<usize> {
        let index = self.by_name.binary_search_by(|&slot| self.properties[usize::from(slot)].name.cmp(name)).ok()?;
        Some(usize::from(self.by_name[index]))
    }

    /// The property that `name` sets: a property, or with `transition` true a
    /// paint property's `<name>-transition`.
    fn resolve(&self, name: &str) -> Option<(usize, bool)> {
        if let Some(slot) = self.find(name) {
            return Some((slot, false));
        }
        let slot = self.find(name.strip_suffix("-transition")?)?;
        (self.properties[slot].scope == PropertyScope::Paint).then_some((slot, true))
    }
}

/// What to do with the field that holds a property, given functions that
/// borrow it from the properties' owner `S`.
pub trait PropertyVisitor<S> {
    type Output;

    fn paint<V: StyleValue>(
        self,
        get: fn(&S) -> &Transitionable<V>,
        get_mut: fn(&mut S) -> &mut Transitionable<V>,
    ) -> Self::Output;

    fn layout<V: StyleValue>(self, get: fn(&S) -> &V, get_mut: fn(&mut S) -> &mut V) -> Self::Output;
}

/// A type that holds the values of the properties in a table.
pub(crate) trait Properties: Clone + 'static {
    const TABLE: &'static PropertyTable;

    /// Calls `visitor` with the field of the property in `slot`, or returns
    /// `None` for a slot past the table's end.
    fn visit<V: PropertyVisitor<Self>>(slot: usize, visitor: V) -> Option<V::Output>;
}

/// Why setting a property failed.
#[derive(Debug)]
pub enum SetPropertyError {
    /// The table has no property of that name.
    Unknown,
    /// The value doesn't convert.
    Invalid(Error),
}

/// The property `name` (or a paint property's `<name>-transition`) of `owner`,
/// undefined for a name the table lacks.
pub(crate) fn get_property<S: Properties>(owner: &S, name: &str) -> StyleProperty {
    match S::TABLE.resolve(name) {
        Some((slot, transition)) => style_property(owner, slot, transition),
        None => StyleProperty::default(),
    }
}

fn style_property<S: Properties>(owner: &S, slot: usize, transition: bool) -> StyleProperty {
    struct Get<'a, S> {
        owner: &'a S,
        transition: bool,
    }

    impl<S> PropertyVisitor<S> for Get<'_, S> {
        type Output = StyleProperty;

        fn paint<V: StyleValue>(
            self,
            get: fn(&S) -> &Transitionable<V>,
            _: fn(&mut S) -> &mut Transitionable<V>,
        ) -> StyleProperty {
            paint_style_property(get(self.owner), self.transition)
        }

        fn layout<V: StyleValue>(self, get: fn(&S) -> &V, _: fn(&mut S) -> &mut V) -> StyleProperty {
            get(self.owner).style_property()
        }
    }

    S::visit(slot, Get { owner, transition }).unwrap_or_default()
}

// The visitors' work is split by the type it depends on: what depends only on
// the value type is in a function of its own, so that it compiles once per
// value type rather than once per value type and owner. And what depends on
// both is not inlined into `Properties::visit`, so that it compiles once per
// pair rather than once per property.

#[inline(never)]
fn paint_style_property<V: StyleValue>(property: &Transitionable<V>, transition: bool) -> StyleProperty {
    if transition { make_transition_style_property(&property.options) } else { property.value.style_property() }
}

/// Where the owner of properties is kept, for [`set_property`] to read it
/// and to change it in place.
pub(crate) trait OwnerSlot<S> {
    /// The owner; `None` when the slot holds something else.
    fn get(&self) -> Option<&S>;

    /// The owner, to change: copied first if it is shared.
    fn make_mut(&mut self) -> Option<&mut S>;
}

impl<S: Clone> OwnerSlot<S> for Arc<S> {
    fn get(&self) -> Option<&S> {
        Some(self)
    }

    #[inline(never)]
    fn make_mut(&mut self) -> Option<&mut S> {
        Some(Arc::make_mut(self))
    }
}

/// An owner that isn't shared, changed in place.
pub(crate) struct InPlace<'a, S>(pub(crate) &'a mut S);

impl<S> OwnerSlot<S> for InPlace<'_, S> {
    fn get(&self) -> Option<&S> {
        Some(self.0)
    }

    fn make_mut(&mut self) -> Option<&mut S> {
        Some(self.0)
    }
}

/// Sets the property `name` (or a paint property's `<name>-transition`) of
/// the owner in `owner` from a style JSON value, whose expressions parse
/// against `functions`. The owner is copied only if the value changes it and
/// it is shared.
pub(crate) fn set_property<S: Properties, O: OwnerSlot<S>>(
    owner: &mut O,
    name: &str,
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
) -> Result<(), SetPropertyError> {
    struct Set<'a, 'v, O> {
        owner: &'a mut O,
        value: &'a Convertible<'v>,
        functions: &'a Arc<Functions>,
        info: &'static PropertyInfo,
        transition: bool,
        error: &'a mut Option<Error>,
    }

    // The change passes through a slot of the caller's, rather than by value,
    // so that this code, compiled for each pair of owner and value type, is
    // only calls.
    impl<S, O: OwnerSlot<S>> PropertyVisitor<S> for Set<'_, '_, O> {
        type Output = ();

        #[inline(never)]
        fn paint<V: StyleValue>(
            self,
            get: fn(&S) -> &Transitionable<V>,
            get_mut: fn(&mut S) -> &mut Transitionable<V>,
        ) {
            let mut change = PaintChange::None;
            let Some(current) = self.owner.get() else { return };
            if paint_change(
                get(current),
                self.value,
                self.functions,
                self.info,
                self.transition,
                &mut change,
                self.error,
            ) && let Some(owner) = self.owner.make_mut()
            {
                apply_paint_change(get_mut(owner), &mut change);
            }
        }

        #[inline(never)]
        fn layout<V: StyleValue>(self, get: fn(&S) -> &V, get_mut: fn(&mut S) -> &mut V) {
            let mut change = None;
            let Some(current) = self.owner.get() else { return };
            if changed_value(get(current), self.value, self.functions, self.info, &mut change, self.error)
                && let Some(owner) = self.owner.make_mut()
            {
                store(get_mut(owner), &mut change);
            }
        }
    }

    let (slot, transition) = S::TABLE.resolve(name).ok_or(SetPropertyError::Unknown)?;
    let info = &S::TABLE.properties[slot];
    let mut error = None;
    S::visit(slot, Set { owner, value, functions, info, transition, error: &mut error })
        .ok_or(SetPropertyError::Unknown)?;
    error.map_or(Ok(()), |error| Err(SetPropertyError::Invalid(error)))
}

/// Sets `change` to `value` converted for the property `info` if it differs
/// from `current`, and returns whether it does. Sets `error` if it doesn't
/// convert.
#[inline(never)]
fn changed_value<V: StyleValue>(
    current: &V,
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
    info: &PropertyInfo,
    change: &mut Option<V>,
    error: &mut Option<Error>,
) -> bool {
    match V::convert(value, info, functions) {
        Ok(value) if *current != value => {
            *change = Some(value);
            true
        }
        Ok(_) => false,
        Err(e) => {
            *error = Some(e);
            false
        }
    }
}

/// What setting a paint property changes.
enum PaintChange<V> {
    None,
    Value(V),
    Options(TransitionOptions),
}

/// Sets `change` to what setting a paint property's value, or with
/// `transition` true its transition, to `value` changes, and returns whether
/// it changes anything. Sets `error` if `value` doesn't convert.
#[inline(never)]
fn paint_change<V: StyleValue>(
    current: &Transitionable<V>,
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
    info: &PropertyInfo,
    transition: bool,
    change: &mut PaintChange<V>,
    error: &mut Option<Error>,
) -> bool {
    if transition {
        let mut conversion_error = Error::default();
        match <TransitionOptions as Converter>::convert(value, &mut conversion_error) {
            Some(options) if current.options != options => *change = PaintChange::Options(options),
            Some(_) => {}
            None => *error = Some(conversion_error),
        }
    } else {
        let mut new_value = None;
        changed_value(&current.value, value, functions, info, &mut new_value, error);
        if let Some(value) = new_value {
            *change = PaintChange::Value(value);
        }
    }
    !matches!(change, PaintChange::None)
}

#[inline(never)]
fn apply_paint_change<V>(property: &mut Transitionable<V>, change: &mut PaintChange<V>) {
    match std::mem::replace(change, PaintChange::None) {
        PaintChange::None => {}
        PaintChange::Value(value) => property.value = value,
        PaintChange::Options(options) => property.options = options,
    }
}

#[inline(never)]
fn store<V>(slot: &mut V, change: &mut Option<V>) {
    if let Some(value) = change.take() {
        *slot = value;
    }
}

/// Whether going from `a` to `b` changes a property that tiles are laid out
/// with: a layout property, a cross-faded paint property, a data-driven
/// paint property while either value differs by feature, or, with
/// `section_overrides`, an overridable paint property's constant.
pub(crate) fn layout_properties_differ<S: Properties>(a: &S, b: &S, section_overrides: bool) -> bool {
    /// Which differences of a property's values count.
    #[derive(Clone, Copy)]
    struct Checks {
        any: bool,
        data_driven: bool,
        constant: bool,
    }

    struct Differs<'a, S> {
        a: &'a S,
        b: &'a S,
        checks: Checks,
    }

    impl<S> PropertyVisitor<S> for Differs<'_, S> {
        type Output = bool;

        fn paint<V: StyleValue>(
            self,
            get: fn(&S) -> &Transitionable<V>,
            _: fn(&mut S) -> &mut Transitionable<V>,
        ) -> bool {
            values_differ(&get(self.a).value, &get(self.b).value, self.checks)
        }

        fn layout<V: StyleValue>(self, get: fn(&S) -> &V, _: fn(&mut S) -> &mut V) -> bool {
            values_differ(get(self.a), get(self.b), self.checks)
        }
    }

    #[inline(never)]
    fn values_differ<V: StyleValue>(a: &V, b: &V, checks: Checks) -> bool {
        (checks.any && a != b)
            || (checks.data_driven && a.has_data_driven_property_difference(b))
            || (checks.constant && a.constant_differs(b))
    }

    S::TABLE.properties.iter().enumerate().any(|(slot, info)| {
        let checks = Checks {
            any: info.scope == PropertyScope::Layout || info.cross_faded,
            data_driven: info.data_driven,
            constant: section_overrides && info.overridable,
        };
        (checks.any || checks.data_driven || checks.constant)
            && S::visit(slot, Differs { a, b, checks }).unwrap_or_default()
    })
}

/// Each defined property of `owner` and paint property transition, in table
/// order, with its name and scope.
pub(crate) fn defined_properties<S: Properties>(owner: &S) -> Vec<(String, PropertyScope, StyleProperty)> {
    defined_properties_in(S::TABLE, &|slot, transition| style_property(owner, slot, transition))
}

/// `defined_properties` over the `table` of an owner whose property in a slot
/// `get` returns, compiled once rather than for each owner.
fn defined_properties_in(
    table: &PropertyTable,
    get: &dyn Fn(usize, bool) -> StyleProperty,
) -> Vec<(String, PropertyScope, StyleProperty)> {
    let table = table.properties;
    let mut defined = Vec::new();
    let paint = table.iter().enumerate().filter(|(_, info)| info.scope == PropertyScope::Paint);
    let layout = table.iter().enumerate().filter(|(_, info)| info.scope == PropertyScope::Layout);
    let values = paint.clone().map(|(slot, info)| (slot, info, false));
    let transitions = paint.map(|(slot, info)| (slot, info, true));
    let layout = layout.map(|(slot, info)| (slot, info, false));
    for (slot, info, transition) in values.chain(transitions).chain(layout) {
        let property = get(slot, transition);
        if property.get_kind() == Kind::Undefined {
            continue;
        }
        let name = if transition { format!("{}-transition", info.name) } else { info.name.to_owned() };
        defined.push((name, info.scope, property));
    }
    defined
}

/// Writes the defined properties of `owner` into a layer's style JSON `out`,
/// under "paint" and "layout". A key already there stays.
pub(crate) fn serialize_properties<S: Properties>(owner: &S, out: &mut ValueObject) {
    insert_properties(defined_properties(owner), out);
}

fn insert_properties(defined: Vec<(String, PropertyScope, StyleProperty)>, out: &mut ValueObject) {
    for (name, scope, property) in defined {
        let key = match scope {
            PropertyScope::Paint => "paint",
            PropertyScope::Layout => "layout",
        };
        if let Some(Value::Object(existing)) = out.get_mut(key) {
            existing.entry(name).or_insert_with(|| property.get_value().clone());
        } else {
            let mut inner = ValueObject::new();
            inner.insert(name, property.get_value().clone());
            out.insert(key.to_owned(), Value::Object(inner));
        }
    }
}

/// The names `S` accepts: each property, followed by its `-transition` for a
/// paint property.
pub(crate) fn names<S: Properties>() -> impl Iterator<Item = String> {
    S::TABLE.properties.iter().flat_map(|info| {
        let transition = (info.scope == PropertyScope::Paint).then(|| format!("{}-transition", info.name));
        std::iter::once(info.name.to_owned()).chain(transition)
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    #[default]
    Undefined,
    Constant,
    Expression,
    Transition,
}

/// Generic representation of a style property.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleProperty {
    value: Value,
    kind: Kind,
}

impl StyleProperty {
    #[must_use]
    pub(crate) fn new(value: Value, kind: Kind) -> Self {
        StyleProperty { value, kind }
    }

    #[must_use]
    pub fn get_value(&self) -> &Value {
        &self.value
    }

    #[must_use]
    pub fn get_kind(&self) -> Kind {
        self.kind
    }
}
