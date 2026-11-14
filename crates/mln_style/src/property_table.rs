//! The properties of a layer type, the light or the sky, as one table each,
//! and the generic access by style-spec name that the style JSON, the runtime
//! and the C API use.
//!
//! A table lists each property's name, whether it is paint or layout, and how
//! its value converts. A property's slot is its index in the table. The type
//! that holds the values implements [`Properties`], which maps a slot to the
//! field that holds it, typed, so that reading, converting and comparing a
//! value needs no downcast.

use std::sync::Arc;

use feature::{Value, ValueObject};
use mln_conversion::{Convertible, Error};

use crate::color_ramp_property_value::ColorRampPropertyValue;
use crate::conversion::property_value::MaybeConvertTokens;
use crate::conversion::{Converter, convert_color_ramp_property_value, convert_property_value};
use crate::conversion_impl::{
    ValueFactory, make_color_ramp_style_property, make_style_property, make_transition_style_property,
};
use crate::expression::ValueConverter;
use crate::layer::PropertyScope;
use crate::properties::Transitionable;
use crate::property_value::PropertyValue;
use crate::style_property::{Kind, StyleProperty};
use crate::transition_options::TransitionOptions;

/// One property in a [`PropertyTable`].
#[derive(Debug)]
pub(crate) struct PropertyInfo {
    /// The style-spec name.
    pub(crate) name: &'static str,
    pub(crate) scope: PropertyScope,
    /// Whether the value may be an expression that reads the feature.
    pub(crate) data_driven: bool,
    /// Whether a string constant's `{token}`s become an expression.
    pub(crate) tokens: bool,
}

impl PropertyInfo {
    #[must_use]
    pub(crate) const fn paint(name: &'static str) -> Self {
        PropertyInfo { name, scope: PropertyScope::Paint, data_driven: false, tokens: false }
    }

    #[must_use]
    pub(crate) const fn layout(name: &'static str) -> Self {
        PropertyInfo { name, scope: PropertyScope::Layout, data_driven: false, tokens: false }
    }

    #[must_use]
    pub(crate) const fn data_driven(self) -> Self {
        PropertyInfo { data_driven: true, ..self }
    }

    #[must_use]
    pub(crate) const fn tokens(self) -> Self {
        PropertyInfo { tokens: true, ..self }
    }
}

/// The properties of one layer type, the light or the sky, in the style
/// spec's order: paint properties, then layout properties.
#[derive(Debug)]
pub(crate) struct PropertyTable {
    properties: &'static [PropertyInfo],
    /// The slots, sorted by property name.
    by_name: &'static [u8],
}

impl PropertyTable {
    /// `by_name` lists the slots of `properties` sorted by name.
    #[must_use]
    pub(crate) const fn new(properties: &'static [PropertyInfo], by_name: &'static [u8]) -> Self {
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

/// A style value a property holds: a [`PropertyValue`], or a color ramp.
pub(crate) trait StyleValue: Clone + Default + PartialEq + 'static {
    /// Converts a style JSON value for the property `info`.
    fn convert(value: &Convertible<'_>, info: &PropertyInfo) -> Result<Self, Error>;

    fn style_property(&self) -> StyleProperty;
}

impl<T> StyleValue for PropertyValue<T>
where
    T: Converter + MaybeConvertTokens + ValueConverter + ValueFactory + Clone + PartialEq + 'static,
{
    fn convert(value: &Convertible<'_>, info: &PropertyInfo) -> Result<Self, Error> {
        let mut error = Error::default();
        convert_property_value::<T>(value, &mut error, info.data_driven, info.tokens).ok_or(error)
    }

    fn style_property(&self) -> StyleProperty {
        make_style_property(self)
    }
}

impl StyleValue for ColorRampPropertyValue {
    fn convert(value: &Convertible<'_>, info: &PropertyInfo) -> Result<Self, Error> {
        let mut error = Error::default();
        convert_color_ramp_property_value(value, &mut error, info.data_driven, info.tokens).ok_or(error)
    }

    fn style_property(&self) -> StyleProperty {
        make_color_ramp_style_property(self)
    }
}

/// What to do with the field that holds a property, given functions that
/// borrow it from the properties' owner `S`.
pub(crate) trait PropertyVisitor<S> {
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

/// Why [`set_property`] failed.
#[derive(Debug)]
pub(crate) enum SetPropertyError {
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
            let property = get(self.owner);
            if self.transition {
                make_transition_style_property(&property.options)
            } else {
                property.value.style_property()
            }
        }

        fn layout<V: StyleValue>(self, get: fn(&S) -> &V, _: fn(&mut S) -> &mut V) -> StyleProperty {
            get(self.owner).style_property()
        }
    }

    S::visit(slot, Get { owner, transition }).unwrap_or_default()
}

/// Sets the property `name` (or a paint property's `<name>-transition`) of
/// `owner` from a style JSON value. `owner` is copied only if the value
/// changes it and it is shared.
pub(crate) fn set_property<S: Properties>(
    owner: &mut Arc<S>,
    name: &str,
    value: &Convertible<'_>,
) -> Result<(), SetPropertyError> {
    struct Set<'a, 'v, S> {
        owner: &'a mut Arc<S>,
        value: &'a Convertible<'v>,
        info: &'static PropertyInfo,
        transition: bool,
    }

    impl<S: Clone> PropertyVisitor<S> for Set<'_, '_, S> {
        type Output = Result<(), Error>;

        fn paint<V: StyleValue>(
            self,
            get: fn(&S) -> &Transitionable<V>,
            get_mut: fn(&mut S) -> &mut Transitionable<V>,
        ) -> Result<(), Error> {
            if self.transition {
                let mut error = Error::default();
                let options = <TransitionOptions as Converter>::convert(self.value, &mut error).ok_or(error)?;
                if get(self.owner).options != options {
                    get_mut(Arc::make_mut(self.owner)).options = options;
                }
            } else {
                let value = V::convert(self.value, self.info)?;
                if get(self.owner).value != value {
                    get_mut(Arc::make_mut(self.owner)).value = value;
                }
            }
            Ok(())
        }

        fn layout<V: StyleValue>(self, get: fn(&S) -> &V, get_mut: fn(&mut S) -> &mut V) -> Result<(), Error> {
            let value = V::convert(self.value, self.info)?;
            if *get(self.owner) != value {
                *get_mut(Arc::make_mut(self.owner)) = value;
            }
            Ok(())
        }
    }

    let (slot, transition) = S::TABLE.resolve(name).ok_or(SetPropertyError::Unknown)?;
    let info = &S::TABLE.properties[slot];
    S::visit(slot, Set { owner, value, info, transition })
        .ok_or(SetPropertyError::Unknown)?
        .map_err(SetPropertyError::Invalid)?;
    Ok(())
}

/// Each defined property of `owner` and paint property transition, in table
/// order, with its name and scope.
pub(crate) fn defined_properties<S: Properties>(owner: &S) -> Vec<(String, PropertyScope, StyleProperty)> {
    let table = S::TABLE.properties;
    let mut defined = Vec::new();
    let paint = table.iter().enumerate().filter(|(_, info)| info.scope == PropertyScope::Paint);
    let layout = table.iter().enumerate().filter(|(_, info)| info.scope == PropertyScope::Layout);
    let values = paint.clone().map(|(slot, info)| (slot, info, false));
    let transitions = paint.map(|(slot, info)| (slot, info, true));
    let layout = layout.map(|(slot, info)| (slot, info, false));
    for (slot, info, transition) in values.chain(transitions).chain(layout) {
        let property = style_property(owner, slot, transition);
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
    for (name, scope, property) in defined_properties(owner) {
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
