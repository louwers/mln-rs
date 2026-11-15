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

extern crate self as mln_wire;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

pub use mln_wire_derive::Wire;

/// Where values are written: bytes, and the first value that couldn't be.
#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
    error: Option<String>,
}

impl Writer {
    #[must_use]
    pub fn new() -> Self {
        Writer::default()
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Writer { bytes: Vec::with_capacity(capacity), error: None }
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

/// Reads values from the start of a slice, and keeps why one couldn't be.
pub struct Reader<'a> {
    bytes: &'a [u8],
    error: Option<&'static str>,
}

const END: &str = "unexpected end of input";

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, error: None }
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

/// The value that fills `bytes`.
pub fn from_slice<T: Wire>(bytes: &[u8]) -> Result<T, Error> {
    let mut r = Reader::new(bytes);
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

/// In full, as serde's `rc` feature writes it: values that cross once and
/// then by key opt in with `mln_util::serialization::shared`.
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

/// serde through a [`Wire`] value's encoding, written as serde bytes: for a
/// `#[serde(with)]` field of a serde type, or a serde impl that delegates.
/// Reading borrows the bytes, which postcard reading a slice allows.
pub mod serde {
    use serde::de::Error as _;
    use serde::ser::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::Wire;

    pub fn serialize<T: Wire, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&super::to_vec(value).map_err(S::Error::custom)?)
    }

    pub fn deserialize<'de, T: Wire, D: Deserializer<'de>>(deserializer: D) -> Result<T, D::Error> {
        let bytes = <&'de [u8]>::deserialize(deserializer)?;
        super::from_slice(bytes).map_err(D::Error::custom)
    }

    /// Implements `Serialize` and `Deserialize` for a [`Wire`] type
    /// through [`serialize`] and [`deserialize`]: `serde_via_wire!(Type)`,
    /// or `serde_via_wire!(for<T: Wire> Type<T>)`.
    #[macro_export]
    macro_rules! serde_via_wire {
        (for<$($generic:ident: $bound:path),+> $t:ty) => {
            impl<$($generic: $bound),+> ::serde::Serialize for $t {
                fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> ::core::result::Result<S::Ok, S::Error> {
                    $crate::serde::serialize(self, serializer)
                }
            }

            impl<'de, $($generic: $bound),+> ::serde::Deserialize<'de> for $t {
                fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> ::core::result::Result<Self, D::Error> {
                    $crate::serde::deserialize(deserializer)
                }
            }
        };
        ($t:ty) => {
            impl ::serde::Serialize for $t {
                fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> ::core::result::Result<S::Ok, S::Error> {
                    $crate::serde::serialize(self, serializer)
                }
            }

            impl<'de> ::serde::Deserialize<'de> for $t {
                fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> ::core::result::Result<Self, D::Error> {
                    $crate::serde::deserialize(deserializer)
                }
            }
        };
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
}
