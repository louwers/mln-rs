//! Port-only: values behind an `Arc` that cross once, then by key
//! (serialization/mod.rs).
//!
//! The map side and a worker instance exchange many messages that refer to
//! the same values: every tile's `setLayers` has the style's layers and
//! their expressions, and every layout the worker sends back has buckets
//! whose binders hold those expressions again. A `Kind` names such a value
//! type, and the fields that hold one opt in with `serialize`/`deserialize`
//! (`#[serde(with = ...)]`). While a codec has its `Tables` open
//! (`Tables::open`), a value is written as follows:
//!
//! - On the side the value comes from (`Tables::origin`), the first time as
//!   a definition under a new key, then as the key. That side keeps the key
//!   while the value lives (a weak reference), so the other side can refer
//!   to the value by it too, and lists the keys of the values that died
//!   (`forget_dead`) for the other side to drop.
//! - On the side that gets it (`Tables::copies`), the definition makes the
//!   value and keeps it under its key until it is forgotten. Sending that
//!   value back writes its key, and the origin reads its own value again.
//!   A value of this side's own is written in full.
//!
//! Outside an open session, or for a kind without a table, every value is
//! written in full.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A type of value that crosses once, then by key.
pub trait Kind: 'static {
    /// The value behind the `Arc`.
    type Value: ?Sized + 'static;
    /// The table's name, which the lists of keys to forget use.
    const NAME: &'static str;
    /// Writes the value in full.
    fn write<S: Serializer>(value: &Self::Value, serializer: S) -> Result<S::Ok, S::Error>;
    /// Reads what `write` wrote.
    fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<Self::Value>, D::Error>;
}

/// A value the side it comes from gave a key, and that key.
type Keyed<T> = (Weak<T>, u64);

/// The keys the side a kind's values come from gave them.
struct Origin<T: ?Sized> {
    by_address: HashMap<usize, Keyed<T>>,
    by_key: HashMap<u64, Weak<T>>,
    /// The keys of values that died and whose address a new value took
    /// before `forget_dead` saw them.
    dead: Vec<u64>,
    /// The keys given since the last `commit`, which `rollback` takes back.
    uncommitted: Vec<(usize, Option<Keyed<T>>)>,
    next: u64,
}

/// The values the other side defined, by key.
struct Copies<T: ?Sized> {
    by_key: HashMap<u64, Arc<T>>,
    keys: HashMap<usize, u64>,
}

fn address<T: ?Sized>(value: &Arc<T>) -> usize {
    Arc::as_ptr(value).cast::<()>() as usize
}

impl<T: ?Sized> Origin<T> {
    /// The value's key, and whether it is new (the other side doesn't have
    /// the value yet).
    fn key(&mut self, value: &Arc<T>) -> (u64, bool) {
        let address = address(value);
        if let Some((weak, key)) = self.by_address.get(&address)
            && weak.upgrade().is_some_and(|known| Arc::ptr_eq(&known, value))
        {
            return (*key, false);
        }
        let key = self.next;
        self.next += 1;
        let replaced = self.by_address.insert(address, (Arc::downgrade(value), key));
        self.by_key.insert(key, Arc::downgrade(value));
        self.uncommitted.push((address, replaced));
        (key, true)
    }

    fn commit(&mut self) {
        for (_, replaced) in self.uncommitted.drain(..) {
            if let Some((_, dead)) = replaced {
                self.dead.push(dead);
            }
        }
    }

    fn rollback(&mut self) {
        for (address, replaced) in self.uncommitted.drain(..).rev() {
            if let Some((_, key)) = self.by_address.remove(&address) {
                self.by_key.remove(&key);
            }
            if let Some(replaced) = replaced {
                self.by_address.insert(address, replaced);
            }
        }
    }

    fn forget_dead(&mut self) -> Vec<u64> {
        let mut forget = std::mem::take(&mut self.dead);
        self.by_address.retain(|_, (weak, key)| {
            let alive = weak.strong_count() > 0;
            if !alive {
                forget.push(*key);
            }
            alive
        });
        for key in &forget {
            self.by_key.remove(key);
        }
        forget.sort_unstable();
        forget
    }
}

/// A table of either side, as `Tables` keeps it.
trait Table: Any + Send {
    fn commit(&mut self);
    fn rollback(&mut self);
    fn forget_dead(&mut self) -> Vec<u64>;
    fn forget(&mut self, keys: &[u64]);
}

impl<T: ?Sized + Send + Sync + 'static> Table for Origin<T> {
    fn commit(&mut self) {
        Origin::commit(self);
    }
    fn rollback(&mut self) {
        Origin::rollback(self);
    }
    fn forget_dead(&mut self) -> Vec<u64> {
        Origin::forget_dead(self)
    }
    fn forget(&mut self, _keys: &[u64]) {}
}

impl<T: ?Sized + Send + Sync + 'static> Table for Copies<T> {
    fn commit(&mut self) {}
    fn rollback(&mut self) {}
    fn forget_dead(&mut self) -> Vec<u64> {
        Vec::new()
    }
    fn forget(&mut self, keys: &[u64]) {
        for key in keys {
            if let Some(value) = self.by_key.remove(key) {
                self.keys.remove(&address(&value));
            }
        }
    }
}

/// One side's tables, one per kind it shares.
#[derive(Default)]
pub struct Tables(Vec<(&'static str, Box<dyn Table>)>);

thread_local! {
    /// The tables of the session `Tables::open` runs.
    static SESSION: RefCell<Option<Tables>> = const { RefCell::new(None) };
}

impl Tables {
    #[must_use]
    pub fn new() -> Self {
        Tables::default()
    }

    /// Adds the table of a kind whose values come from this side.
    #[must_use]
    pub fn origin<K: Kind>(mut self) -> Self
    where
        K::Value: Send + Sync,
    {
        let table = Origin::<K::Value> {
            by_address: HashMap::new(),
            by_key: HashMap::new(),
            dead: Vec::new(),
            uncommitted: Vec::new(),
            next: 0,
        };
        self.0.push((K::NAME, Box::new(table)));
        self
    }

    /// Adds the table of a kind whose values the other side defines.
    #[must_use]
    pub fn copies<K: Kind>(mut self) -> Self
    where
        K::Value: Send + Sync,
    {
        self.0.push((K::NAME, Box::new(Copies::<K::Value> { by_key: HashMap::new(), keys: HashMap::new() })));
        self
    }

    /// Runs `f`, which encodes or decodes a message, with these tables. On
    /// the origin side, the keys given while it runs hold if `f` returns
    /// `Ok` and are taken back otherwise, so that a message that wasn't sent
    /// defines nothing.
    pub fn open<R, E>(&mut self, f: impl FnOnce() -> Result<R, E>) -> Result<R, E> {
        let tables = std::mem::take(self);
        let outer = SESSION.with(|session| session.borrow_mut().replace(tables));
        let result = f();
        *self = SESSION.with(|session| std::mem::replace(&mut *session.borrow_mut(), outer)).unwrap_or_default();
        for (_, table) in &mut self.0 {
            if result.is_ok() { table.commit() } else { table.rollback() }
        }
        result
    }

    /// The keys of the values that died on this side, by table, for the
    /// other side to forget; tables without any are left out.
    pub fn forget_dead(&mut self) -> Vec<(String, Vec<u64>)> {
        self.0
            .iter_mut()
            .filter_map(|(name, table)| {
                let keys = table.forget_dead();
                (!keys.is_empty()).then(|| (name.to_string(), keys))
            })
            .collect()
    }

    /// Drops the copies of the values the other side lists as dead
    /// (`forget_dead`).
    pub fn forget(&mut self, dead: &[(String, Vec<u64>)]) {
        for (name, keys) in dead {
            if let Some((_, table)) = self.0.iter_mut().find(|(table, _)| table == name) {
                table.forget(keys);
            }
        }
    }

    fn table<K: Kind>(&mut self) -> Option<&mut dyn Any> {
        self.0.iter_mut().find(|(name, _)| *name == K::NAME).map(|(_, table)| -> &mut dyn Any { &mut **table })
    }
}

/// Calls `f` with the open session's table of `K`, if there is one.
fn with_table<K: Kind, R>(f: impl FnOnce(TableRef<'_, K::Value>) -> R) -> Option<R> {
    SESSION.with(|session| {
        let mut session = session.borrow_mut();
        let table = session.as_mut()?.table::<K>()?;
        if let Some(origin) = table.downcast_mut::<Origin<K::Value>>() {
            return Some(f(TableRef::Origin(origin)));
        }
        table.downcast_mut::<Copies<K::Value>>().map(|copies| f(TableRef::Copies(copies)))
    })
}

enum TableRef<'a, T: ?Sized> {
    Origin(&'a mut Origin<T>),
    Copies(&'a mut Copies<T>),
}

/// How a value is written.
#[derive(Serialize)]
#[serde(bound = "")]
enum Written<'a, K: Kind> {
    /// The first time from the origin: the key, and the value in full.
    Define(u64, #[serde(serialize_with = "write_value::<K, _>")] &'a K::Value),
    /// The key either side knows the value by.
    Ref(u64),
    /// The value in full, without a key.
    Inline(#[serde(serialize_with = "write_value::<K, _>")] &'a K::Value),
}

#[derive(Deserialize)]
#[serde(bound = "")]
enum Read<K: Kind> {
    Define(u64, #[serde(deserialize_with = "K::read")] Arc<K::Value>),
    Ref(u64),
    Inline(#[serde(deserialize_with = "K::read")] Arc<K::Value>),
}

fn write_value<K: Kind, S: Serializer>(value: &&K::Value, serializer: S) -> Result<S::Ok, S::Error> {
    K::write(value, serializer)
}

/// Writes a value of kind `K` (`#[serde(serialize_with)]`).
pub fn serialize<K: Kind, S: Serializer>(value: &Arc<K::Value>, serializer: S) -> Result<S::Ok, S::Error> {
    enum How {
        Define(u64),
        Ref(u64),
        Inline,
    }
    let how = with_table::<K, _>(|table| match table {
        TableRef::Origin(origin) => match origin.key(value) {
            (key, true) => How::Define(key),
            (key, false) => How::Ref(key),
        },
        TableRef::Copies(copies) => copies.keys.get(&address(value)).map_or(How::Inline, |key| How::Ref(*key)),
    });
    match how.unwrap_or(How::Inline) {
        How::Define(key) => Written::<K>::Define(key, value).serialize(serializer),
        How::Ref(key) => Written::<K>::Ref(key).serialize(serializer),
        How::Inline => Written::<K>::Inline(value).serialize(serializer),
    }
}

/// Reads a value of kind `K` (`#[serde(deserialize_with)]`).
pub fn deserialize<'de, K: Kind, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<K::Value>, D::Error> {
    let (key, value) = read::<K, D>(deserializer)?;
    value.ok_or_else(|| D::Error::custom(format!("no {} {}", K::NAME, key.unwrap_or_default())))
}

/// Reads a value of kind `K` as `deserialize` does, but a key this side
/// doesn't know (any more) reads as `None` rather than as an error.
pub fn deserialize_known<'de, K: Kind, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Arc<K::Value>>, D::Error> {
    Ok(read::<K, D>(deserializer)?.1)
}

/// A value read, if any, and the key it was read by, if any.
type KeyedRead<T> = (Option<u64>, Option<Arc<T>>);

/// The value read, and the key it was read by, if any.
fn read<'de, K: Kind, D: Deserializer<'de>>(deserializer: D) -> Result<KeyedRead<K::Value>, D::Error> {
    Ok(match Read::<K>::deserialize(deserializer)? {
        Read::Define(key, value) => {
            with_table::<K, _>(|table| {
                if let TableRef::Copies(copies) = table {
                    copies.keys.insert(address(&value), key);
                    copies.by_key.insert(key, Arc::clone(&value));
                }
            });
            (Some(key), Some(value))
        }
        Read::Ref(key) => {
            let value = with_table::<K, _>(|table| match table {
                TableRef::Origin(origin) => origin.by_key.get(&key).and_then(Weak::upgrade),
                TableRef::Copies(copies) => copies.by_key.get(&key).cloned(),
            });
            (Some(key), value.flatten())
        }
        Read::Inline(value) => (None, Some(value)),
    })
}

/// Writes an `Option` of a value of kind `K`.
pub fn serialize_option<K: Kind, S: Serializer>(
    value: &Option<Arc<K::Value>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    struct Some<'a, K: Kind>(&'a Arc<K::Value>);
    impl<K: Kind> Serialize for Some<'_, K> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serialize::<K, S>(self.0, serializer)
        }
    }
    value.as_ref().map(Some::<K>).serialize(serializer)
}

/// Reads an `Option` of a value of kind `K`.
pub fn deserialize_option<'de, K: Kind, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Arc<K::Value>>, D::Error> {
    struct Value<K: Kind>(Arc<K::Value>);
    impl<'de, K: Kind> Deserialize<'de> for Value<K> {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserialize::<K, D>(deserializer).map(Value)
        }
    }
    Ok(Option::<Value<K>>::deserialize(deserializer)?.map(|value| value.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Text;

    impl Kind for Text {
        type Value = str;
        const NAME: &'static str = "text";
        fn write<S: Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
            value.serialize(serializer)
        }
        fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<str>, D::Error> {
            String::deserialize(deserializer).map(Arc::from)
        }
    }

    #[derive(Serialize, Deserialize)]
    struct Message(
        #[serde(serialize_with = "serialize::<Text, _>", deserialize_with = "deserialize::<Text, _>")] Arc<str>,
    );

    fn encode(tables: &mut Tables, value: &Arc<str>) -> Vec<u8> {
        tables.open(|| postcard::to_allocvec(&Message(Arc::clone(value)))).unwrap()
    }

    fn decode(tables: &mut Tables, bytes: &[u8]) -> Result<Arc<str>, postcard::Error> {
        tables.open(|| postcard::from_bytes::<Message>(bytes)).map(|message| message.0)
    }

    /// A value crosses in full once, then by key, both ways; the side it
    /// comes from reads its own value back.
    #[test]
    fn values_cross_once() {
        let mut origin = Tables::new().origin::<Text>();
        let mut copies = Tables::new().copies::<Text>();
        let value: Arc<str> = Arc::from("a value long enough to tell apart from a key");

        let first = encode(&mut origin, &value);
        let second = encode(&mut origin, &value);
        assert!(first.len() > value.len() && second.len() < 4);
        let copy = decode(&mut copies, &first).unwrap();
        assert_eq!(&*copy, &*value);
        assert!(Arc::ptr_eq(&decode(&mut copies, &second).unwrap(), &copy));

        let back = encode(&mut copies, &copy);
        assert_eq!(back, second);
        assert!(Arc::ptr_eq(&decode(&mut origin, &back).unwrap(), &value));

        // A value of the copies' side's own crosses in full.
        let own: Arc<str> = Arc::from("own");
        assert_eq!(&*decode(&mut origin, &encode(&mut copies, &own)).unwrap(), "own");
    }

    /// The copies forget what died at the origin, and a message that wasn't
    /// sent defines nothing.
    #[test]
    fn dead_and_unsent_values() {
        let mut origin = Tables::new().origin::<Text>();
        let mut copies = Tables::new().copies::<Text>();
        let value: Arc<str> = Arc::from("value");
        let reference = {
            decode(&mut copies, &encode(&mut origin, &value)).unwrap();
            encode(&mut origin, &value)
        };
        drop(value);
        let dead = origin.forget_dead();
        assert_eq!(dead, vec![("text".to_owned(), vec![0])]);
        copies.forget(&dead);
        assert!(decode(&mut copies, &reference).is_err());

        let unsent: Arc<str> = Arc::from("unsent");
        let failed: Result<(), ()> = origin.open(|| {
            postcard::to_allocvec(&Message(Arc::clone(&unsent))).unwrap();
            Err(())
        });
        assert!(failed.is_err());
        let sent = encode(&mut origin, &unsent);
        assert_eq!(&*decode(&mut copies, &sent).unwrap(), "unsent");
    }
}
