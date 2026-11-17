//! Port-only: values behind an `Arc` that cross once, then by key
//! (serialization/mod.rs).
//!
//! The map side and a worker instance exchange many messages that refer to
//! the same values: every tile's layout job has its source's style, its
//! layers and their expressions, and every layout the worker sends back has
//! buckets whose binders hold those expressions again. A [`Kind`] names such
//! a value type, and the fields that hold one opt in with [`ByKey`]
//! (`#[wire(with = ...)]`) or [`write()`] and [`read()`]. Each side keeps a
//! [`Table`] of each kind it shares, and while a message is written or read
//! with a kind's table in its context, a value is written as follows:
//!
//! - On the side the value comes from ([`Role::Origin`]), the first time as
//!   a definition under a new key, then as the key. That side keeps the key
//!   while the value lives (a weak reference), so the other side can refer
//!   to the value by it too, and lists the keys of the values that died
//!   ([`ErasedTable::forget_dead`]) for the other side to drop.
//! - On the side that gets it ([`Role::Copies`]), the definition makes the
//!   value and keeps it under its key until it is forgotten. Sending that
//!   value back writes its key, and the origin reads its own value again.
//!   A value of this side's own is written in full.
//!
//! Without its kind's table in the context, a value is written in full.

use std::marker::PhantomData;
use std::sync::{Arc, Weak};

use mln_wire::{Reader, Writer};

use crate::hash::HashMap;

/// A type of value that crosses once, then by key, and how it is written
/// in full.
pub trait Kind: 'static {
    /// The value behind the `Arc`.
    type Value: ?Sized + Send + Sync + 'static;
    /// Writes the value in full.
    fn write(value: &Self::Value, w: &mut Writer);
    /// Reads what `write` wrote.
    fn read(r: &mut Reader<'_>) -> Option<Arc<Self::Value>>;
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

/// Which side of a kind's values a table is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The side the values come from.
    Origin,
    /// The side that gets them.
    Copies,
}

/// One side's table of the values of kind `K`. A message written or read
/// with it in its context ([`mln_wire::Context`]) refers to them by key.
pub struct Table<K: Kind>(Side<K::Value>);

enum Side<T: ?Sized> {
    Origin(Origin<T>),
    Copies(Copies<T>),
}

impl<K: Kind> Table<K> {
    #[must_use]
    pub fn new(role: Role) -> Self {
        Table(match role {
            Role::Origin => Side::Origin(Origin {
                by_address: HashMap::default(),
                by_key: HashMap::default(),
                dead: Vec::new(),
                uncommitted: Vec::new(),
                next: 0,
            }),
            Role::Copies => Side::Copies(Copies { by_key: HashMap::default(), keys: HashMap::default() }),
        })
    }

    /// How `value` is written, giving it a key if it is new.
    fn how(&mut self, value: &Arc<K::Value>) -> How {
        match &mut self.0 {
            Side::Origin(origin) => match origin.key(value) {
                (key, true) => How::Define(key),
                (key, false) => How::Ref(key),
            },
            Side::Copies(copies) => copies.keys.get(&address(value)).map_or(How::Inline, |key| How::Ref(*key)),
        }
    }

    /// Keeps a value the other side defined under `key`.
    fn define(&mut self, key: u64, value: &Arc<K::Value>) {
        if let Side::Copies(copies) = &mut self.0 {
            copies.keys.insert(address(value), key);
            copies.by_key.insert(key, Arc::clone(value));
        }
    }

    /// The value either side knows by `key`.
    fn by_key(&self, key: u64) -> Option<Arc<K::Value>> {
        match &self.0 {
            Side::Origin(origin) => origin.by_key.get(&key).and_then(Weak::upgrade),
            Side::Copies(copies) => copies.by_key.get(&key).cloned(),
        }
    }
}

impl<K: Kind> mln_wire::Context for Table<K> {
    fn provide<'a>(&'a mut self, request: &mut mln_wire::Request<'a>) {
        request.give_mut(self);
    }
}

/// A table of any kind, as a side keeps its tables together.
pub trait ErasedTable: mln_wire::Context + Send {
    /// Keeps the keys given since the last `commit` or `rollback`: the
    /// message that gave them was sent.
    fn commit(&mut self);
    /// Takes back the keys given since the last `commit` or `rollback`, so
    /// that a message that wasn't sent defines nothing.
    fn rollback(&mut self);
    /// The keys of the values that died on the origin side, for the other
    /// side to forget.
    fn forget_dead(&mut self) -> Vec<u64>;
    /// Drops the copies of the values the origin lists as dead.
    fn forget(&mut self, keys: &[u64]);
}

impl<K: Kind> ErasedTable for Table<K> {
    fn commit(&mut self) {
        if let Side::Origin(origin) = &mut self.0 {
            origin.commit();
        }
    }

    fn rollback(&mut self) {
        if let Side::Origin(origin) = &mut self.0 {
            origin.rollback();
        }
    }

    fn forget_dead(&mut self) -> Vec<u64> {
        match &mut self.0 {
            Side::Origin(origin) => origin.forget_dead(),
            Side::Copies(_) => Vec::new(),
        }
    }

    fn forget(&mut self, keys: &[u64]) {
        if let Side::Copies(copies) = &mut self.0 {
            for key in keys {
                if let Some(value) = copies.by_key.remove(key) {
                    copies.keys.remove(&address(&value));
                }
            }
        }
    }
}

/// How a value is written, as the variant index it is written with.
enum How {
    /// The first time from the origin: the key, and the value in full.
    Define(u64),
    /// The key either side knows the value by.
    Ref(u64),
    /// The value in full, without a key.
    Inline,
}

/// Writes a value of kind `K`.
pub fn write<K: Kind>(value: &Arc<K::Value>, w: &mut Writer) {
    let how = w.context_mut::<Table<K>>().map_or(How::Inline, |table| table.how(value));
    match how {
        How::Define(key) => {
            w.push(0);
            mln_wire::write_varint(w, key);
            K::write(value, w);
        }
        How::Ref(key) => {
            w.push(1);
            mln_wire::write_varint(w, key);
        }
        How::Inline => {
            w.push(2);
            K::write(value, w);
        }
    }
}

/// Reads what `write` wrote.
pub fn read<K: Kind>(r: &mut Reader<'_>) -> Option<Arc<K::Value>> {
    match r.byte()? {
        0 => {
            let key = r.varint()?;
            let value = K::read(r)?;
            if let Some(table) = r.context_mut::<Table<K>>() {
                table.define(key, &value);
            }
            Some(value)
        }
        1 => {
            let key = r.varint()?;
            let value = r.context_mut::<Table<K>>().and_then(|table| table.by_key(key));
            value.or_else(|| r.fail("a value by a key this side doesn't know"))
        }
        2 => K::read(r),
        _ => r.fail_tag(),
    }
}

/// Writes an `Option` of a value of kind `K`.
pub fn write_option<K: Kind>(value: &Option<Arc<K::Value>>, w: &mut Writer) {
    match value {
        None => w.push(0),
        Some(value) => {
            w.push(1);
            write::<K>(value, w);
        }
    }
}

/// Reads what `write_option` wrote.
pub fn read_option<K: Kind>(r: &mut Reader<'_>) -> Option<Option<Arc<K::Value>>> {
    match r.byte()? {
        0 => Some(None),
        1 => read::<K>(r).map(Some),
        _ => r.fail("invalid option"),
    }
}

/// `#[wire(with = "shared::ByKey::<K>")]` for an `Arc` of a value of kind
/// `K`: [`write()`] and [`read()`].
pub struct ByKey<K>(PhantomData<K>);

impl<K: Kind> ByKey<K> {
    pub fn write(value: &Arc<K::Value>, w: &mut Writer) {
        write::<K>(value, w);
    }

    pub fn read(r: &mut Reader<'_>) -> Option<Arc<K::Value>> {
        read::<K>(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Text;

    impl Kind for Text {
        type Value = str;

        fn write(value: &str, w: &mut Writer) {
            mln_wire::write_str(w, value);
        }

        fn read(r: &mut Reader<'_>) -> Option<Arc<str>> {
            mln_wire::read_str(r).map(Arc::from)
        }
    }

    fn encode(table: &mut Table<Text>, value: &Arc<str>) -> Vec<u8> {
        let mut w = Writer::new().with_context(table);
        write::<Text>(value, &mut w);
        let bytes = w.finish().unwrap();
        table.commit();
        bytes
    }

    fn decode(table: &mut Table<Text>, bytes: &[u8]) -> Option<Arc<str>> {
        read::<Text>(&mut Reader::new(bytes).with_context(table))
    }

    /// A value crosses in full once, then by key, both ways; the side it
    /// comes from reads its own value back.
    #[test]
    fn values_cross_once() {
        let mut origin = Table::<Text>::new(Role::Origin);
        let mut copies = Table::<Text>::new(Role::Copies);
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

        // Without the table in the context, a value crosses in full.
        let mut w = Writer::new();
        write::<Text>(&value, &mut w);
        assert_eq!(read::<Text>(&mut Reader::new(&w.finish().unwrap())).as_deref(), Some(&*value));
    }

    /// The copies forget what died at the origin, and a message that wasn't
    /// sent defines nothing.
    #[test]
    fn dead_and_unsent_values() {
        let mut origin = Table::<Text>::new(Role::Origin);
        let mut copies = Table::<Text>::new(Role::Copies);
        let value: Arc<str> = Arc::from("value");
        let reference = {
            decode(&mut copies, &encode(&mut origin, &value)).unwrap();
            encode(&mut origin, &value)
        };
        drop(value);
        let dead = origin.forget_dead();
        assert_eq!(dead, vec![0]);
        copies.forget(&dead);
        assert!(decode(&mut copies, &reference).is_none());

        let unsent: Arc<str> = Arc::from("unsent");
        write::<Text>(&unsent, &mut Writer::new().with_context(&mut origin));
        origin.rollback();
        let sent = encode(&mut origin, &unsent);
        assert_eq!(&*decode(&mut copies, &sent).unwrap(), "unsent");
    }
}
