//! Extension members: members of the style's root or of a layer named
//! `<namespace>:<name>`, which no style spec member is, and which the style
//! spec leaves to extensions.
//!
//! A style keeps every such member as the JSON value it was given, writes it
//! back out and lets the set calls change it. An extension declares a
//! [`Member`], which gives the member's value a type: a style parses the
//! value of a member its registry has once, and readers get the parsed value
//! ([`Members::get`]). A member the registry lacks is kept as it is, with a
//! warning.

use std::any::Any;
use std::sync::Arc;

use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::feature::{Value, ValueObject};

/// How a [`Member`]'s value parses from style JSON, whose expressions
/// parse against the functions.
pub type Parse<T> = fn(&Convertible<'_>, &Arc<Functions>) -> Result<T, Error>;

/// Where a member is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// The style's root.
    Root,
    /// Every layer, of any type.
    Layer,
}

/// An extension's member of the style's root or of every layer, which it
/// declares as a `static` and registers with the engine's registry.
///
/// ```
/// use mln_style::conversion::Error;
/// use mln_style::member::Member;
///
/// pub struct Group(pub String);
///
/// pub static GROUP: Member<Group> = Member::layer("org.example:group", |value, _| {
///     value.to_string().map(Group).ok_or_else(|| Error::new("expected a group's name"))
/// });
/// ```
pub struct Member<T> {
    name: &'static str,
    place: Place,
    parse: Parse<T>,
}

impl<T: Send + Sync + 'static> Member<T> {
    /// A member of the style's root, whose value `parse` reads.
    ///
    /// # Panics
    ///
    /// If `name` isn't `<namespace>:<name>`. In a `static`, that is a
    /// compile error.
    #[must_use]
    pub const fn root(name: &'static str, parse: Parse<T>) -> Self {
        Member::new(name, Place::Root, parse)
    }

    /// A member of every layer, as [`root`](Self::root) makes one of the
    /// style's root.
    ///
    /// # Panics
    ///
    /// As [`root`](Self::root) does.
    #[must_use]
    pub const fn layer(name: &'static str, parse: Parse<T>) -> Self {
        Member::new(name, Place::Layer, parse)
    }

    const fn new(name: &'static str, place: Place, parse: Parse<T>) -> Self {
        assert!(is_extension_member(name), "an extension member's name is `<namespace>:<name>`");
        Member { name, place, parse }
    }
}

impl<T> Member<T> {
    /// The member's name, as style JSON spells it.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub const fn place(&self) -> Place {
        self.place
    }
}

/// Whether `name` is an extension member's: it has a `:` with something
/// before and after it.
#[must_use]
pub const fn is_extension_member(name: &str) -> bool {
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

type Parsed = Arc<dyn Any + Send + Sync>;

/// A [`Member`] of any value type.
pub trait ErasedMember: Send + Sync {
    fn name(&self) -> &'static str;

    fn place(&self) -> Place;

    /// Parses a value of the member.
    ///
    /// # Errors
    ///
    /// When the value isn't one of the member's.
    fn parse(&self, value: &Convertible<'_>, functions: &Arc<Functions>) -> Result<Parsed, Error>;
}

impl<T: Send + Sync + 'static> ErasedMember for Member<T> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn place(&self) -> Place {
        self.place
    }

    fn parse(&self, value: &Convertible<'_>, functions: &Arc<Functions>) -> Result<Parsed, Error> {
        (self.parse)(value, functions).map(|value| {
            let parsed: Parsed = Arc::new(value);
            parsed
        })
    }
}

/// The extension members of the style's root or of a layer, by name: each
/// as the JSON value it was given, and for a registered member, as its
/// parsed value.
#[derive(Clone, Default)]
pub struct Members(Vec<Entry>);

#[derive(Clone)]
struct Entry {
    name: Arc<str>,
    json: Value,
    /// `None` for a member the registry lacks.
    parsed: Option<Parsed>,
}

impl Members {
    /// The parsed value of `member`, if it is set.
    #[must_use]
    pub fn get<T: 'static>(&self, member: &Member<T>) -> Option<&T> {
        let entry = &self.0[self.find(member.name).ok()?];
        entry.parsed.as_deref()?.downcast_ref()
    }

    /// The JSON value of the member `name`, if it is set.
    #[must_use]
    pub fn json(&self, name: &str) -> Option<&Value> {
        Some(&self.0[self.find(name).ok()?].json)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Sets the member `name` to `json`, parsed as `parsed`, or with `None`
    /// unsets it.
    pub(crate) fn set(&mut self, name: &str, value: Option<(Value, Option<Parsed>)>) {
        match (self.find(name), value) {
            (Ok(index), Some((json, parsed))) => {
                let entry = &mut self.0[index];
                entry.json = json;
                entry.parsed = parsed;
            }
            (Ok(index), None) => {
                self.0.remove(index);
            }
            (Err(index), Some((json, parsed))) => self.0.insert(index, Entry { name: Arc::from(name), json, parsed }),
            (Err(_), None) => {}
        }
    }

    /// Writes each member into `out` under its name.
    pub(crate) fn serialize(&self, out: &mut ValueObject) {
        for entry in &self.0 {
            out.insert(entry.name.to_string(), entry.json.clone());
        }
    }

    fn find(&self, name: &str) -> Result<usize, usize> {
        self.0.binary_search_by(|entry| (*entry.name).cmp(name))
    }
}

/// Members are equal when their JSON values are: a member's parsed value
/// follows from its JSON.
impl PartialEq for Members {
    fn eq(&self, other: &Members) -> bool {
        self.0.len() == other.0.len() && self.0.iter().zip(&other.0).all(|(a, b)| a.name == b.name && a.json == b.json)
    }
}

impl std::fmt::Debug for Members {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.0.iter().map(|entry| (&entry.name, &entry.json))).finish()
    }
}

/// `value` as a [`Value`], undefined as null.
pub(crate) fn to_value(value: &Convertible<'_>) -> Value {
    if value.is_undefined() {
        Value::Null
    } else if value.is_array() {
        Value::Array((0..value.array_length()).map(|i| to_value(&value.array_member(i))).collect())
    } else if value.is_object() {
        let mut object = ValueObject::new();
        value.each_member(|name, member| {
            object.insert(name.to_owned(), to_value(member));
            None
        });
        Value::Object(object)
    } else {
        value.to_value().unwrap_or(Value::Null)
    }
}
