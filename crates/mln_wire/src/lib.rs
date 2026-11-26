//! Port-only: a compact binary encoding for the values that cross between
//! wasm instances (`mln_renderer`'s `tile/worker_codec`).
//!
//! A [`Wire`] type writes itself to a byte vector and reads itself from a
//! [`Reader`] over a slice. Both are concrete, so each type compiles to one
//! write function and one read function, without the visitors and error
//! plumbing a serde format compiles for each type. The encoding is
//! postcard's: unsigned integers as LEB128 varints, signed ones zigzagged
//! first, floats as their little-endian bytes, a sequence as its length and
//! its items, an `Option` as a byte 0 or 1, a struct as its fields in order
//! and an enum as its variant index and then its fields.
//!
//! `#[derive(Wire)]` writes the impl for a struct or an enum
//! ([`mln_wire_derive`]).
//!
//! A value that needs more than its bytes to cross, such as the registry
//! that a name resolves against, gets it from the [`Context`] that the
//! writer or reader was made with.
//!
//! Fields opt in to how some values cross:
//!
//! - [`payload`]: large byte arrays (pixels, vertices, indices) travel beside
//!   a message rather than in it, so that a host can transfer them.
//! - [`shared`]: a value behind an `Arc` that several messages refer to (a
//!   layer, an expression, an image) travels once per pair of sides and then
//!   by key.
//! - [`sorted`]: hash maps and sets in key order, so that a value always
//!   encodes to the same bytes.
//!
//! The first two only change how a value is written when the message is
//! written with their tables in its context; otherwise the value is written
//! in full, in the message.

extern crate self as mln_wire;

#[cfg(target_family = "wasm")]
pub mod host_bytes;
pub mod payload;
pub mod shared;
pub mod sorted;

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

pub use mln_wire_derive::Wire;

/// What values need beyond their bytes to cross: the tables of the values
/// that crossed before, the registry that names resolve against. A value
/// asks its writer or reader for the part it needs by the part's type
/// ([`Writer::context`], [`Reader::context`]).
///
/// The parts' types belong to the crates that use them, which this crate
/// can't name, and the writer and reader are concrete types, since trait
/// objects write and read themselves through them. So a context gives out
/// its parts by type, and [`Request`] is the one place that erases them.
///
/// Contexts combine as tuples, which ask each member in turn.
pub trait Context {
    /// Gives `request` the part it asks for, if this context has it.
    fn provide<'a>(&'a mut self, request: &mut Request<'a>);
}

/// A request for the part of a [`Context`] of one type.
pub struct Request<'a> {
    wanted: TypeId,
    exclusive: bool,
    found: Option<Part<'a>>,
}

enum Part<'a> {
    Shared(&'a dyn Any),
    Exclusive(&'a mut dyn Any),
}

impl<'a> Request<'a> {
    fn new<T: Any>(exclusive: bool) -> Self {
        Request { wanted: TypeId::of::<T>(), exclusive, found: None }
    }

    /// Gives `part`, if the request is for a `T` it can share.
    pub fn give<T: Any>(&mut self, part: &'a T) -> &mut Self {
        if self.found.is_none() && !self.exclusive && self.wanted == TypeId::of::<T>() {
            self.found = Some(Part::Shared(part));
        }
        self
    }

    /// Gives `part`, if the request is for a `T`.
    pub fn give_mut<T: Any>(&mut self, part: &'a mut T) -> &mut Self {
        if self.found.is_none() && self.wanted == TypeId::of::<T>() {
            self.found = Some(Part::Exclusive(part));
        }
        self
    }

    fn shared<T: Any>(self) -> Option<&'a T> {
        match self.found? {
            Part::Shared(part) => part.downcast_ref(),
            Part::Exclusive(part) => part.downcast_ref(),
        }
    }

    fn exclusive<T: Any>(self) -> Option<&'a mut T> {
        match self.found? {
            Part::Shared(_) => None,
            Part::Exclusive(part) => part.downcast_mut(),
        }
    }
}

/// The part of `cx` of type `T`.
fn part<'a, T: Any>(cx: Option<&'a mut (dyn Context + '_)>) -> Option<&'a T> {
    let mut request = Request::new::<T>(false);
    cx?.provide(&mut request);
    request.shared()
}

/// The part of `cx` of type `T`, to change.
fn part_mut<'a, T: Any>(cx: Option<&'a mut (dyn Context + '_)>) -> Option<&'a mut T> {
    let mut request = Request::new::<T>(true);
    cx?.provide(&mut request);
    request.exclusive()
}

/// No parts.
impl Context for () {
    fn provide<'a>(&'a mut self, _: &mut Request<'a>) {}
}

impl<C: Context + ?Sized> Context for &mut C {
    fn provide<'a>(&'a mut self, request: &mut Request<'a>) {
        (**self).provide(request);
    }
}

impl<A: Context, B: Context> Context for (A, B) {
    fn provide<'a>(&'a mut self, request: &mut Request<'a>) {
        self.0.provide(request);
        self.1.provide(request);
    }
}

impl<A: Context, B: Context, C: Context> Context for (A, B, C) {
    fn provide<'a>(&'a mut self, request: &mut Request<'a>) {
        self.0.provide(request);
        self.1.provide(request);
        self.2.provide(request);
    }
}

/// Where values are written: bytes, the first value that couldn't be, and
/// the context they are written in.
#[derive(Default)]
pub struct Writer<'cx> {
    bytes: Vec<u8>,
    error: Option<String>,
    context: Option<&'cx mut (dyn Context + 'cx)>,
}

impl Writer<'_> {
    #[must_use]
    pub fn new() -> Self {
        Writer::default()
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Writer { bytes: Vec::with_capacity(capacity), error: None, context: None }
    }

    /// A writer that appends to `bytes`.
    #[must_use]
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        Writer { bytes, error: None, context: None }
    }

    /// This writer, writing in `context`.
    #[must_use]
    pub fn with_context<'c>(self, context: &'c mut (dyn Context + 'c)) -> Writer<'c> {
        Writer { bytes: self.bytes, error: self.error, context: Some(context) }
    }

    /// The context's part of type `T`, if it has one.
    pub fn context<T: Any>(&mut self) -> Option<&T> {
        part(self.context.as_deref_mut())
    }

    /// The context's part of type `T`, to change, if it has one.
    pub fn context_mut<T: Any>(&mut self) -> Option<&mut T> {
        part_mut(self.context.as_deref_mut())
    }

    #[inline]
    pub fn push(&mut self, byte: u8) {
        self.bytes.push(byte);
    }

    #[inline]
    pub fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Records that a value can't be written; the first such error is what
    /// [`Writer::finish`] returns.
    #[cold]
    pub fn fail(&mut self, error: String) {
        self.error.get_or_insert(error);
    }

    /// The bytes written, or why a value couldn't be.
    pub fn finish(self) -> Result<Vec<u8>, String> {
        match self.error {
            None => Ok(self.bytes),
            Some(error) => Err(error),
        }
    }
}

/// Why a value couldn't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(&'static str);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Error {}

/// A value that crosses between wasm instances.
///
/// `read` returns `None` when the value can't be read, after it records why
/// in the reader ([`Reader::fail`]): an `Option` takes less code to pass up
/// through the reads of a value's parts than a `Result` does.
pub trait Wire: Sized {
    fn write(&self, w: &mut Writer);
    fn read(r: &mut Reader<'_>) -> Option<Self>;
}

/// Reads values from the start of a slice, in a context, and keeps why one
/// couldn't be.
pub struct Reader<'a> {
    bytes: &'a [u8],
    error: Option<&'static str>,
    context: Option<&'a mut (dyn Context + 'a)>,
}

const END: &str = "unexpected end of input";

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, error: None, context: None }
    }

    /// This reader, reading in `context`.
    #[must_use]
    pub fn with_context(self, context: &'a mut (dyn Context + 'a)) -> Self {
        Reader { context: Some(context), ..self }
    }

    /// The context's part of type `T`, if it has one.
    pub fn context<T: Any>(&mut self) -> Option<&T> {
        part(self.context.as_deref_mut())
    }

    /// The context's part of type `T`, to change, if it has one.
    pub fn context_mut<T: Any>(&mut self) -> Option<&mut T> {
        part_mut(self.context.as_deref_mut())
    }

    /// The bytes not read yet.
    #[must_use]
    pub fn rest(&self) -> &'a [u8] {
        self.bytes
    }

    /// Records why a value can't be read, and returns `None` for the read to
    /// return.
    #[cold]
    pub fn fail<T>(&mut self, message: &'static str) -> Option<T> {
        self.error.get_or_insert(message);
        None
    }

    /// Why a read returned `None`.
    #[must_use]
    pub fn error(&self) -> Error {
        Error(self.error.unwrap_or("malformed"))
    }

    #[inline]
    pub fn byte(&mut self) -> Option<u8> {
        match self.bytes.split_first() {
            Some((&byte, rest)) => {
                self.bytes = rest;
                Some(byte)
            }
            None => self.fail(END),
        }
    }

    /// The next `n` bytes.
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.bytes.len() {
            return self.fail(END);
        }
        let (taken, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Some(taken)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        match self.bytes.split_first_chunk::<N>() {
            Some((taken, rest)) => {
                self.bytes = rest;
                Some(*taken)
            }
            None => self.fail(END),
        }
    }

    /// A varint.
    #[inline(never)]
    pub fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            // The tenth byte holds the one bit left of a `u64`.
            if shift == 63 && byte > 1 {
                return self.fail("varint too long");
            }
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        self.fail("varint too long")
    }

    /// An enum's variant index.
    pub fn tag(&mut self) -> Option<u32> {
        let tag = self.varint()?;
        u32::try_from(tag).ok().or_else(|| self.fail_tag())
    }

    /// Records an enum variant index the enum doesn't have.
    #[cold]
    pub fn fail_tag<T>(&mut self) -> Option<T> {
        self.fail("unknown variant")
    }

    /// A sequence's length.
    pub fn length(&mut self) -> Option<usize> {
        let len = self.varint()?;
        usize::try_from(len).ok().or_else(|| self.fail("length too large"))
    }
}

/// Reads a `T` into `slot`, dropping what it held. Not inlined, so that a
/// struct of many fields (`#[wire(default)]`) calls it once per field rather
/// than holding each field's read, move and drop.
#[inline(never)]
pub fn read_into<T: Wire>(slot: &mut T, r: &mut Reader<'_>) -> Option<()> {
    *slot = T::read(r)?;
    Some(())
}

/// Writes a varint.
#[inline(never)]
pub fn write_varint(w: &mut Writer, mut value: u64) {
    while value >= 0x80 {
        w.push((value as u8) | 0x80);
        value >>= 7;
    }
    w.push(value as u8);
}

/// Writes an enum's variant index.
#[inline]
pub fn write_tag(w: &mut Writer, tag: u32) {
    if tag < 0x80 {
        w.push(tag as u8);
    } else {
        write_varint(w, u64::from(tag));
    }
}

fn write_len(w: &mut Writer, len: usize) {
    write_varint(w, len as u64);
}

/// `value` encoded, or why it can't be.
pub fn to_vec<T: Wire>(value: &T) -> Result<Vec<u8>, String> {
    let mut w = Writer::new();
    value.write(&mut w);
    w.finish()
}

/// `value` encoded in `context`, or why it can't be.
pub fn to_vec_in<T: Wire>(value: &T, mut context: impl Context) -> Result<Vec<u8>, String> {
    let mut w = Writer::new().with_context(&mut context);
    value.write(&mut w);
    w.finish()
}

/// The value that fills `bytes`.
pub fn from_slice<T: Wire>(bytes: &[u8]) -> Result<T, Error> {
    from_slice_in(bytes, ())
}

/// The value that fills `bytes`, read in `context`.
pub fn from_slice_in<T: Wire>(bytes: &[u8], mut context: impl Context) -> Result<T, Error> {
    let mut r = Reader::new(bytes).with_context(&mut context);
    match T::read(&mut r) {
        Some(_) if !r.rest().is_empty() => Err(Error("bytes after the end")),
        Some(value) => Ok(value),
        None => Err(r.error()),
    }
}

impl Wire for u8 {
    fn write(&self, w: &mut Writer) {
        w.push(*self);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        r.byte()
    }
}

impl Wire for i8 {
    fn write(&self, w: &mut Writer) {
        w.push(*self as u8);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(r.byte()? as i8)
    }
}

impl Wire for bool {
    fn write(&self, w: &mut Writer) {
        w.push(u8::from(*self));
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        match r.byte()? {
            0 => Some(false),
            1 => Some(true),
            _ => r.fail("invalid bool"),
        }
    }
}

macro_rules! unsigned {
    ($($t:ty: $to_u64:expr),+) => {$(
        impl Wire for $t {
            fn write(&self, w: &mut Writer) {
                write_varint(w, $to_u64(*self));
            }
            fn read(r: &mut Reader<'_>) -> Option<Self> {
                let value = r.varint()?;
                <$t>::try_from(value).ok().or_else(|| r.fail("integer out of range"))
            }
        }
    )+};
}
unsigned!(u16: u64::from, u32: u64::from, u64: std::convert::identity, usize: |value: usize| value as u64);

macro_rules! signed {
    ($($t:ty: $to_i64:expr),+) => {$(
        impl Wire for $t {
            fn write(&self, w: &mut Writer) {
                let value: i64 = $to_i64(*self);
                write_varint(w, ((value << 1) ^ (value >> 63)) as u64);
            }
            fn read(r: &mut Reader<'_>) -> Option<Self> {
                let value = r.varint()?;
                let value = ((value >> 1) as i64) ^ -((value & 1) as i64);
                <$t>::try_from(value).ok().or_else(|| r.fail("integer out of range"))
            }
        }
    )+};
}
signed!(i16: i64::from, i32: i64::from, i64: std::convert::identity, isize: |value: isize| value as i64);

macro_rules! non_zero {
    ($($t:ty),+) => {$(
        /// As the integer, which can't be zero.
        impl Wire for std::num::NonZero<$t> {
            fn write(&self, w: &mut Writer) {
                self.get().write(w);
            }
            fn read(r: &mut Reader<'_>) -> Option<Self> {
                let value = <$t>::read(r)?;
                Self::new(value).or_else(|| r.fail("zero for a non-zero integer"))
            }
        }
    )+};
}
non_zero!(u8, u16, u32, u64, usize);

impl Wire for f32 {
    fn write(&self, w: &mut Writer) {
        w.extend_from_slice(&self.to_le_bytes());
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(f32::from_le_bytes(r.array()?))
    }
}

impl Wire for f64 {
    fn write(&self, w: &mut Writer) {
        w.extend_from_slice(&self.to_le_bytes());
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(f64::from_le_bytes(r.array()?))
    }
}

impl Wire for char {
    fn write(&self, w: &mut Writer) {
        write_varint(w, u64::from(u32::from(*self)));
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        let value = r.varint()?;
        u32::try_from(value).ok().and_then(char::from_u32).or_else(|| r.fail("invalid char"))
    }
}

impl Wire for () {
    fn write(&self, _: &mut Writer) {}
    fn read(_: &mut Reader<'_>) -> Option<Self> {
        Some(())
    }
}

impl<T: ?Sized> Wire for std::marker::PhantomData<T> {
    fn write(&self, _: &mut Writer) {}
    fn read(_: &mut Reader<'_>) -> Option<Self> {
        Some(std::marker::PhantomData)
    }
}

/// Writes a string.
pub fn write_str(w: &mut Writer, value: &str) {
    write_len(w, value.len());
    w.extend_from_slice(value.as_bytes());
}

/// Reads a string, borrowed from the input.
pub fn read_str<'a>(r: &mut Reader<'a>) -> Option<&'a str> {
    let len = r.length()?;
    let bytes = r.take(len)?;
    std::str::from_utf8(bytes).ok().or_else(|| r.fail("invalid UTF-8"))
}

impl Wire for String {
    fn write(&self, w: &mut Writer) {
        write_str(w, self);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        read_str(r).map(str::to_owned)
    }
}

impl Wire for Box<str> {
    fn write(&self, w: &mut Writer) {
        write_str(w, self);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        read_str(r).map(Box::from)
    }
}

impl Wire for Arc<str> {
    fn write(&self, w: &mut Writer) {
        write_str(w, self);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        read_str(r).map(Arc::from)
    }
}

/// Writes a sequence's length and its items.
pub fn write_seq<'a, T: Wire + 'a>(w: &mut Writer, items: &'a [T]) {
    write_len(w, items.len());
    for item in items {
        item.write(w);
    }
}

/// Reads a sequence's length and its items.
pub fn read_seq<T: Wire>(r: &mut Reader<'_>) -> Option<Vec<T>> {
    let len = r.length()?;
    // A malformed length doesn't reserve more than the input could hold.
    let mut items = Vec::with_capacity(len.min(r.rest().len()));
    for _ in 0..len {
        items.push(T::read(r)?);
    }
    Some(items)
}

impl<T: Wire> Wire for Vec<T> {
    fn write(&self, w: &mut Writer) {
        write_seq(w, self);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        read_seq(r)
    }
}

impl<T: Wire> Wire for Box<[T]> {
    fn write(&self, w: &mut Writer) {
        write_seq(w, self);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        read_seq(r).map(Vec::into_boxed_slice)
    }
}

impl<T: Wire> Wire for Box<T> {
    fn write(&self, w: &mut Writer) {
        (**self).write(w);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        T::read(r).map(Box::new)
    }
}

/// In full: values that cross once and then by key opt in with [`shared`].
impl<T: Wire> Wire for Arc<T> {
    fn write(&self, w: &mut Writer) {
        (**self).write(w);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        T::read(r).map(Arc::new)
    }
}

impl<T: Wire> Wire for Option<T> {
    fn write(&self, w: &mut Writer) {
        match self {
            None => w.push(0),
            Some(value) => {
                w.push(1);
                value.write(w);
            }
        }
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        match r.byte()? {
            0 => Some(None),
            1 => T::read(r).map(Some),
            _ => r.fail("invalid option"),
        }
    }
}

impl<T: Wire + Default + Copy, const N: usize> Wire for [T; N] {
    fn write(&self, w: &mut Writer) {
        for item in self {
            item.write(w);
        }
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        let mut items = [T::default(); N];
        for item in &mut items {
            *item = T::read(r)?;
        }
        Some(items)
    }
}

macro_rules! tuple {
    ($($name:ident $index:tt),+) => {
        impl<$($name: Wire),+> Wire for ($($name,)+) {
            fn write(&self, w: &mut Writer) {
                $(self.$index.write(w);)+
            }
            fn read(r: &mut Reader<'_>) -> Option<Self> {
                Some(($($name::read(r)?,)+))
            }
        }
    };
}
tuple!(A 0);
tuple!(A 0, B 1);
tuple!(A 0, B 1, C 2);
tuple!(A 0, B 1, C 2, D 3);

impl<K: Wire + Ord, V: Wire> Wire for BTreeMap<K, V> {
    fn write(&self, w: &mut Writer) {
        write_len(w, self.len());
        for (key, value) in self {
            key.write(w);
            value.write(w);
        }
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        let len = r.length()?;
        let mut map = BTreeMap::new();
        for _ in 0..len {
            let key = K::read(r)?;
            map.insert(key, V::read(r)?);
        }
        Some(map)
    }
}

/// Hash maps and sets of any hasher, in iteration order, which differs
/// between maps of the same entries: [`sorted`] writes one in key order.
/// And the map of this crate's own tables.
mod hash {
    #![expect(clippy::disallowed_types, reason = "impls for every hasher, mln_style's among them")]

    use std::collections::HashSet;
    use std::hash::{BuildHasher, Hash};

    use super::{Reader, Wire, Writer, write_len};

    /// foldhash's hasher, with a fixed seed in the benchmarks' builds, as
    /// `mln_style::hash`'s.
    #[cfg(not(mln_bench_fixed_hash))]
    type Hasher = foldhash::fast::RandomState;
    #[cfg(mln_bench_fixed_hash)]
    type Hasher = foldhash::fast::FixedState;

    /// std's map, with foldhash's hasher for this crate's own tables.
    pub(crate) type HashMap<K, V, S = Hasher> = std::collections::HashMap<K, V, S>;

    impl<K: Wire + Eq + Hash, V: Wire, S: BuildHasher + Default> Wire for HashMap<K, V, S> {
        fn write(&self, w: &mut Writer) {
            write_len(w, self.len());
            for (key, value) in self {
                key.write(w);
                value.write(w);
            }
        }
        fn read(r: &mut Reader<'_>) -> Option<Self> {
            let len = r.length()?;
            let mut map = HashMap::with_capacity_and_hasher(len.min(r.rest().len()), S::default());
            for _ in 0..len {
                let key = K::read(r)?;
                map.insert(key, V::read(r)?);
            }
            Some(map)
        }
    }

    impl<T: Wire + Eq + Hash, S: BuildHasher + Default> Wire for HashSet<T, S> {
        fn write(&self, w: &mut Writer) {
            write_len(w, self.len());
            for value in self {
                value.write(w);
            }
        }
        fn read(r: &mut Reader<'_>) -> Option<Self> {
            let len = r.length()?;
            let mut set = HashSet::with_capacity_and_hasher(len.min(r.rest().len()), S::default());
            for _ in 0..len {
                set.insert(T::read(r)?);
            }
            Some(set)
        }
    }
}

impl<T: Wire + Ord> Wire for BTreeSet<T> {
    fn write(&self, w: &mut Writer) {
        write_len(w, self.len());
        for value in self {
            value.write(w);
        }
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        let len = r.length()?;
        let mut set = BTreeSet::new();
        for _ in 0..len {
            set.insert(T::read(r)?);
        }
        Some(set)
    }
}

impl Wire for Duration {
    fn write(&self, w: &mut Writer) {
        self.as_secs().write(w);
        self.subsec_nanos().write(w);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        let secs = u64::read(r)?;
        let nanos = u32::read(r)?;
        if nanos >= 1_000_000_000 {
            return r.fail("invalid duration");
        }
        Some(Duration::new(secs, nanos))
    }
}

#[cfg(feature = "glam")]
impl Wire for glam::Vec2 {
    fn write(&self, w: &mut Writer) {
        self.x.write(w);
        self.y.write(w);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        let x = f32::read(r)?;
        Some(glam::Vec2::new(x, f32::read(r)?))
    }
}

/// The value behind the lock, which writing locks.
#[cfg(feature = "parking_lot")]
impl<T: Wire> Wire for parking_lot::RwLock<T> {
    fn write(&self, w: &mut Writer) {
        self.read().write(w);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        T::read(r).map(parking_lot::RwLock::new)
    }
}

/// The value behind the lock, which writing locks.
#[cfg(feature = "parking_lot")]
impl<T: Wire> Wire for parking_lot::Mutex<T> {
    fn write(&self, w: &mut Writer) {
        self.lock().write(w);
    }
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        T::read(r).map(parking_lot::Mutex::new)
    }
}

/// `geo_types`' geometries, field by field.
#[cfg(feature = "geo-types")]
mod geo {
    use geo_types::{
        Coord, CoordNum, Geometry, GeometryCollection, Line, LineString, MultiLineString, MultiPoint, MultiPolygon,
        Point, Polygon, Rect, Triangle,
    };

    use super::{Reader, Wire, Writer, write_tag};

    impl<T: CoordNum + Wire> Wire for Coord<T> {
        fn write(&self, w: &mut Writer) {
            self.x.write(w);
            self.y.write(w);
        }
        fn read(r: &mut Reader<'_>) -> Option<Self> {
            let x = T::read(r)?;
            Some(Coord { x, y: T::read(r)? })
        }
    }

    impl<T: CoordNum + Wire> Wire for Point<T> {
        fn write(&self, w: &mut Writer) {
            self.0.write(w);
        }
        fn read(r: &mut Reader<'_>) -> Option<Self> {
            Coord::read(r).map(Point)
        }
    }

    impl<T: CoordNum + Wire> Wire for LineString<T> {
        fn write(&self, w: &mut Writer) {
            self.0.write(w);
        }
        fn read(r: &mut Reader<'_>) -> Option<Self> {
            Vec::read(r).map(LineString)
        }
    }

    impl<T: CoordNum + Wire> Wire for Polygon<T> {
        fn write(&self, w: &mut Writer) {
            self.exterior().write(w);
            super::write_seq(w, self.interiors());
        }
        fn read(r: &mut Reader<'_>) -> Option<Self> {
            let exterior = LineString::read(r)?;
            Some(Polygon::new(exterior, Vec::read(r)?))
        }
    }

    impl<T: CoordNum + Wire> Wire for Geometry<T> {
        fn write(&self, w: &mut Writer) {
            match self {
                Geometry::Point(point) => {
                    write_tag(w, 0);
                    point.write(w);
                }
                Geometry::Line(line) => {
                    write_tag(w, 1);
                    line.start.write(w);
                    line.end.write(w);
                }
                Geometry::LineString(line) => {
                    write_tag(w, 2);
                    line.write(w);
                }
                Geometry::Polygon(polygon) => {
                    write_tag(w, 3);
                    polygon.write(w);
                }
                Geometry::MultiPoint(points) => {
                    write_tag(w, 4);
                    points.0.write(w);
                }
                Geometry::MultiLineString(lines) => {
                    write_tag(w, 5);
                    lines.0.write(w);
                }
                Geometry::MultiPolygon(polygons) => {
                    write_tag(w, 6);
                    polygons.0.write(w);
                }
                Geometry::GeometryCollection(geometries) => {
                    write_tag(w, 7);
                    geometries.0.write(w);
                }
                Geometry::Rect(rect) => {
                    write_tag(w, 8);
                    rect.min().write(w);
                    rect.max().write(w);
                }
                Geometry::Triangle(triangle) => {
                    write_tag(w, 9);
                    triangle.v1().write(w);
                    triangle.v2().write(w);
                    triangle.v3().write(w);
                }
            }
        }
        fn read(r: &mut Reader<'_>) -> Option<Self> {
            Some(match r.tag()? {
                0 => Geometry::Point(Point::read(r)?),
                1 => {
                    let start = Coord::read(r)?;
                    Geometry::Line(Line::new(start, Coord::read(r)?))
                }
                2 => Geometry::LineString(LineString::read(r)?),
                3 => Geometry::Polygon(Polygon::read(r)?),
                4 => Geometry::MultiPoint(MultiPoint(Vec::read(r)?)),
                5 => Geometry::MultiLineString(MultiLineString(Vec::read(r)?)),
                6 => Geometry::MultiPolygon(MultiPolygon(Vec::read(r)?)),
                7 => Geometry::GeometryCollection(GeometryCollection(Vec::read(r)?)),
                8 => {
                    let min = Coord::read(r)?;
                    Geometry::Rect(Rect::new(min, Coord::read(r)?))
                }
                9 => {
                    let a = Coord::read(r)?;
                    let b = Coord::read(r)?;
                    // As written: the writer's winding.
                    Geometry::Triangle(Triangle::unchecked_winding(a, b, Coord::read(r)?))
                }
                _ => return r.fail_tag(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Wire)]
    struct Named<T> {
        a: u32,
        b: Option<T>,
        #[wire(skip)]
        c: u8,
    }

    #[derive(Debug, PartialEq, Wire)]
    enum Shape {
        Empty,
        Point(f32, f32),
        Named { name: String, sides: Vec<i32> },
    }

    #[test]
    fn values_round_trip() {
        let named = Named { a: 300, b: Some(Shape::Named { name: "x".into(), sides: vec![-1, 2, -300] }), c: 0 };
        let bytes = to_vec(&named).unwrap();
        assert_eq!(from_slice::<Named<Shape>>(&bytes).unwrap(), named);
        for shape in [Shape::Empty, Shape::Point(1.0, -2.5)] {
            assert_eq!(from_slice::<Shape>(&to_vec(&shape).unwrap()).unwrap(), shape);
        }
    }

    #[test]
    fn truncated_input_is_an_error() {
        let bytes = to_vec(&Shape::Named { name: "abc".into(), sides: vec![1, 2] }).unwrap();
        for end in 0..bytes.len() {
            assert!(from_slice::<Shape>(&bytes[..end]).is_err());
        }
        assert_eq!(from_slice::<Shape>(&[9]), Err(Error("unknown variant")));
    }

    struct Count(u32);

    impl Context for Count {
        fn provide<'a>(&'a mut self, request: &mut Request<'a>) {
            request.give_mut(self);
        }
    }

    struct Name(&'static str);

    impl Context for &Name {
        fn provide<'a>(&'a mut self, request: &mut Request<'a>) {
            request.give::<Name>(self);
        }
    }

    /// Reads as how many it is in the context's count.
    struct Counted(u32);

    impl Wire for Counted {
        fn write(&self, _: &mut Writer) {}
        fn read(r: &mut Reader<'_>) -> Option<Self> {
            let count = r.context_mut::<Count>()?;
            count.0 += 1;
            Some(Counted(count.0))
        }
    }

    /// Values get the context's parts by type, from any member of a tuple,
    /// and a shared part only to read.
    #[test]
    fn values_ask_the_context_for_their_parts() {
        let mut count = Count(0);
        let read: (Counted, Counted) = from_slice_in(&[], (&Name("name"), &mut count)).unwrap();
        assert_eq!((read.0.0, read.1.0, count.0), (1, 2, 2));
        assert!(from_slice::<Counted>(&[]).is_err());

        let mut context = (&Name("name"), ());
        let mut w = Writer::new().with_context(&mut context);
        assert_eq!(w.context::<Name>().map(|name| name.0), Some("name"));
        assert!(w.context_mut::<Name>().is_none());
        assert!(w.context::<Count>().is_none());
    }

    #[test]
    fn varints_past_64_bits_are_errors() {
        let mut max = vec![0xff; 9];
        max.push(0x01);
        assert_eq!(from_slice::<u64>(&max), Ok(u64::MAX));
        let mut over = vec![0x80; 9];
        over.push(0x02);
        assert!(from_slice::<u64>(&over).is_err());
    }
}
