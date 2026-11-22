//! Style keys: members of the style's root, or of a layer beside its
//! `source` and `filter`, that an extension declares ([`StyleKey`]), and the
//! values a style holds for them.

use std::any::{Any, TypeId};
use std::fmt::Debug;
use std::sync::Arc;

use feature::{Value, ValueObject};
use mln_vec_map::VecMap;

use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::expression::functions::RegisterError;
use crate::layer_type::StyleLayerType;
use crate::property_table::PropertyTable;

/// How a [`StyleKey`]'s value parses from style JSON, whose expressions
/// parse against the functions.
pub type Parse<T> = fn(&Convertible<'_>, &Arc<Functions>) -> Result<T, Error>;

/// A member of the style's root, or of a layer beside its `source` and
/// `filter`, that an extension declares as a `static` and registers with
/// `RegistryBuilder::add_style_key`.
///
/// A key is a setting read as a value, not a property: nothing of it
/// transitions or differs by feature. A value may hold expressions of its
/// own type, which its reader evaluates. The style reads a value by its type
/// ([`Style::extra`](crate::Style::extra),
/// [`LayerBase::extra`](crate::layer_type::LayerBase::extra)), so a registry
/// takes one key per value type. A key is the renderer's: a layer reads it
/// in its render half, never in its layout, to which the worker codec sends
/// no key and which an edit of keys alone doesn't run again.
///
/// A key's name is `<namespace>:<key>`, which no style spec member is.
/// Where a member with a `:` in its name isn't a registered key of its place
/// (unregistered, or a key of another place), a style load warns and leaves
/// it out, and adding a layer or setting the member fails. Members without a
/// `:` aren't keys.
///
/// ```
/// use std::sync::Arc;
///
/// use feature::Value;
/// use mln_style::conversion::{Convertible, Error};
/// use mln_style::expression::Functions;
/// use mln_style::style_key::StyleKey;
///
/// #[derive(Debug, PartialEq)]
/// pub struct Group(pub String);
///
/// pub static GROUP: StyleKey<Group> =
///     StyleKey::layer("org.example:group", parse_group, |group| Value::String(group.0.clone()));
///
/// fn parse_group(value: &Convertible<'_>, _: &Arc<Functions>) -> Result<Group, Error> {
///     value.to_string().map(Group).ok_or_else(|| Error::new("expected a group's name"))
/// }
/// ```
pub struct StyleKey<T> {
    name: &'static str,
    scope: Scope,
    parse: Parse<T>,
    to_json: fn(&T) -> Value,
}

/// Where a key is a member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    Root,
    Layer,
    /// Layers of the layer type of this name.
    LayerOf(&'static str),
}

impl<T: PartialEq + Debug + Send + Sync + 'static> StyleKey<T> {
    /// A member of the style's root, which `parse` reads and `to_json`
    /// writes.
    ///
    /// # Panics
    ///
    /// If `name` isn't `<namespace>:<key>`. In a `static`, that is a compile
    /// error.
    #[must_use]
    pub const fn root(name: &'static str, parse: Parse<T>, to_json: fn(&T) -> Value) -> Self {
        StyleKey::new(name, Scope::Root, parse, to_json)
    }

    /// A member of every layer, of any type, as [`root`](Self::root) makes
    /// one of the style's root.
    ///
    /// # Panics
    ///
    /// As [`root`](Self::root) does.
    #[must_use]
    pub const fn layer(name: &'static str, parse: Parse<T>, to_json: fn(&T) -> Value) -> Self {
        StyleKey::new(name, Scope::Layer, parse, to_json)
    }

    /// A member of the layers of the type `S` only, as
    /// [`layer`](Self::layer) makes one of every layer.
    ///
    /// # Panics
    ///
    /// As [`root`](Self::root) does.
    #[must_use]
    pub const fn layer_of<S: StyleLayerType>(name: &'static str, parse: Parse<T>, to_json: fn(&T) -> Value) -> Self {
        StyleKey::new(name, Scope::LayerOf(S::INFO.name), parse, to_json)
    }

    const fn new(name: &'static str, scope: Scope, parse: Parse<T>, to_json: fn(&T) -> Value) -> Self {
        assert!(is_namespaced(name), "a style key's name is `<namespace>:<key>`");
        StyleKey { name, scope, parse, to_json }
    }

    /// The key's name, as style JSON spells it.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

/// Whether `name` has a `:` with something before and after it.
const fn is_namespaced(name: &str) -> bool {
    let bytes = name.as_bytes();
    let mut i = 1;
    while i + 1 < bytes.len() {
        if bytes[i] == b':' {
            return true;
        }
        i += 1;
    }
    false
}

type ExtraValue = Arc<dyn Any + Send + Sync>;

/// A [`StyleKey`] of any value type.
pub(crate) trait AnyKey: Send + Sync {
    fn name(&self) -> &'static str;

    fn scope(&self) -> Scope;

    fn value_type(&self) -> TypeId;

    fn parse(&self, value: &Convertible<'_>, functions: &Arc<Functions>) -> Result<ExtraValue, Error>;

    /// What `value` sets the key to: `None`, which unsets it, for undefined
    /// or null.
    fn value_of(&self, value: &Convertible<'_>, functions: &Arc<Functions>) -> Result<Option<ExtraValue>, Error> {
        if value.is_undefined() { Ok(None) } else { self.parse(value, functions).map(Some) }
    }

    fn to_json(&self, value: &(dyn Any + Send + Sync)) -> Value;

    fn eq(&self, a: &(dyn Any + Send + Sync), b: &(dyn Any + Send + Sync)) -> bool;

    fn fmt(&self, value: &(dyn Any + Send + Sync), f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

impl<T: PartialEq + Debug + Send + Sync + 'static> AnyKey for StyleKey<T> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn scope(&self) -> Scope {
        self.scope
    }

    fn value_type(&self) -> TypeId {
        TypeId::of::<T>()
    }

    fn parse(&self, value: &Convertible<'_>, functions: &Arc<Functions>) -> Result<ExtraValue, Error> {
        match (self.parse)(value, functions) {
            Ok(value) => Ok(Arc::new(value)),
            Err(error) => Err(Error::new(format!("style key \"{}\": {}", self.name, error.message))),
        }
    }

    fn to_json(&self, value: &(dyn Any + Send + Sync)) -> Value {
        value.downcast_ref().map_or(Value::Null, self.to_json)
    }

    fn eq(&self, a: &(dyn Any + Send + Sync), b: &(dyn Any + Send + Sync)) -> bool {
        a.downcast_ref::<T>() == b.downcast_ref::<T>()
    }

    fn fmt(&self, value: &(dyn Any + Send + Sync), f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match value.downcast_ref::<T>() {
            Some(value) => value.fmt(f),
            None => f.write_str("_"),
        }
    }
}

fn same_key(a: &dyn AnyKey, b: &dyn AnyKey) -> bool {
    std::ptr::addr_eq(a, b)
}

/// Where a member is.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Place<'a> {
    Root,
    /// A layer of the type of this name.
    Layer(&'a str),
}

/// The style keys of a registry, by name, and the property tables of its
/// layer types, which no key may share a name with.
#[derive(Default)]
pub struct StyleKeys {
    keys: VecMap<&'static str, &'static dyn AnyKey>,
    properties: Vec<&'static PropertyTable>,
}

impl StyleKeys {
    /// Adds `key`. Adding a key again changes nothing.
    ///
    /// # Errors
    ///
    /// [`RegisterError::Duplicate`] for a name another key or a property of
    /// an added layer type has, or a value type another key has.
    pub fn add<T: PartialEq + Debug + Send + Sync + 'static>(
        &mut self,
        key: &'static StyleKey<T>,
    ) -> Result<(), RegisterError> {
        let name = key.name;
        if let Some(&added) = self.keys.get(name) {
            return if same_key(added, key) { Ok(()) } else { Err(RegisterError::Duplicate(name.to_owned())) };
        }
        if self.keys.values().any(|added| added.value_type() == TypeId::of::<T>()) {
            return Err(RegisterError::Duplicate(format!("a style key of {}", std::any::type_name::<T>())));
        }
        if self.properties.iter().any(|table| table.find(name).is_some()) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.keys.insert(name, key);
        Ok(())
    }

    /// Adds the properties of the layer type `S`, which no key may be named
    /// as.
    ///
    /// # Errors
    ///
    /// [`RegisterError::Duplicate`] for a property named as a key.
    pub fn add_layer_type<S: StyleLayerType>(&mut self) -> Result<(), RegisterError> {
        let table = S::PROPERTIES;
        if let Some(name) = self.keys.keys().find(|name| table.find(name).is_some()) {
            return Err(RegisterError::Duplicate((*name).to_owned()));
        }
        self.properties.push(table);
        Ok(())
    }

    /// The key named `name`, if there is one.
    pub(crate) fn get(&self, name: &str) -> Option<&'static dyn AnyKey> {
        self.keys.get(name).copied()
    }

    /// The key that the member `name` at `place` sets, or why it sets none:
    /// no key has the name, or the key is of another place.
    pub(crate) fn at(&self, name: &str, place: Place<'_>) -> Result<&'static dyn AnyKey, Error> {
        let Some(key) = self.get(name) else {
            return Err(Error::new(format!("no extension registered the style key \"{name}\"")));
        };
        let belongs = match (key.scope(), place) {
            (Scope::Root, Place::Root) | (Scope::Layer, Place::Layer(_)) => return Ok(key),
            (Scope::LayerOf(type_), Place::Layer(layer_type)) if type_ == layer_type => return Ok(key),
            (Scope::Root, Place::Layer(_)) => "at the style's root".to_owned(),
            (Scope::Layer, Place::Root) => "on layers".to_owned(),
            (Scope::LayerOf(type_), _) => format!("on \"{type_}\" layers"),
        };
        Err(Error::new(format!("the style key \"{name}\" belongs {belongs}")))
    }
}

impl Debug for StyleKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.keys.keys()).finish()
    }
}

/// The values that a layer or the style's root holds for style keys, in
/// key name order.
#[derive(Clone, Default)]
pub(crate) struct Extras(Vec<Extra>);

#[derive(Clone)]
struct Extra {
    key: &'static dyn AnyKey,
    value: ExtraValue,
}

impl Extras {
    /// The value of the key whose value type is `T`.
    pub(crate) fn get<T: 'static>(&self) -> Option<&T> {
        self.0.iter().find_map(|extra| extra.value.downcast_ref())
    }

    /// Whether the value of `key` is `value`, or with `None` unset.
    pub(crate) fn holds(&self, key: &dyn AnyKey, value: Option<&ExtraValue>) -> bool {
        match (self.find(key.name()), value) {
            (Ok(index), Some(value)) => {
                let held = &self.0[index].value;
                Arc::ptr_eq(held, value) || key.eq(&**held, &**value)
            }
            (Err(_), None) => true,
            _ => false,
        }
    }

    /// Sets the value of `key`, or with `None` unsets it.
    pub(crate) fn set(&mut self, key: &'static dyn AnyKey, value: Option<ExtraValue>) {
        match (self.find(key.name()), value) {
            (Ok(index), Some(value)) => self.0[index] = Extra { key, value },
            (Ok(index), None) => {
                self.0.remove(index);
            }
            (Err(index), Some(value)) => self.0.insert(index, Extra { key, value }),
            (Err(_), None) => {}
        }
    }

    /// The style JSON value of the key named `name`.
    pub(crate) fn json(&self, name: &str) -> Option<Value> {
        let extra = &self.0[self.find(name).ok()?];
        Some(extra.key.to_json(&*extra.value))
    }

    /// Writes each value into `out` under its key's name.
    pub(crate) fn serialize(&self, out: &mut ValueObject) {
        for extra in &self.0 {
            out.insert(extra.key.name().to_owned(), extra.key.to_json(&*extra.value));
        }
    }

    fn find(&self, name: &str) -> Result<usize, usize> {
        self.0.binary_search_by(|extra| extra.key.name().cmp(name))
    }
}

impl PartialEq for Extras {
    fn eq(&self, other: &Extras) -> bool {
        self.0.len() == other.0.len()
            && self.0.iter().zip(&other.0).all(|(a, b)| {
                same_key(a.key, b.key) && (Arc::ptr_eq(&a.value, &b.value) || a.key.eq(&*a.value, &*b.value))
            })
    }
}

impl Debug for Extras {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct Held<'a>(&'a Extra);
        impl Debug for Held<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.key.fmt(&*self.0.value, f)
            }
        }
        f.debug_map().entries(self.0.iter().map(|extra| (extra.key.name(), Held(extra)))).finish()
    }
}
