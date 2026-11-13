//! Port-only: byte arrays that travel beside a message (serialization/mod.rs).
//!
//! In a message encoded inside `encode`, an array of at least `PAYLOAD_MIN`
//! bytes is not written into the message: it is moved to the session's
//! payloads, and the message refers to it by index. `decode` gives the
//! payloads back to the arrays that refer to them, as they are. Fields opt
//! in with `#[serde(with = "...")]`: `bytes` for a `Vec<u8>`, `shared_bytes`
//! for an `Arc<Vec<u8>>` and `pod` for a `Vec` of plain values (vertices,
//! indices). Outside a session the arrays are written in the message, as
//! serde bytes, which postcard writes in one piece (serde writes a `Vec<u8>`
//! byte by byte).
//!
//! On wasm a host can hand `decode` its payloads as the JS arrays they
//! arrived in (`Payload::Host`). Each field copies its array into wasm
//! memory, but for `pod::deserialize_payload`, whose caller can keep it in
//! JS.

use std::cell::RefCell;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(target_family = "wasm")]
use crate::host_bytes::HostBytes;

/// A payload as `decode` takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Bytes(Vec<u8>),
    /// A JS array, outside wasm memory.
    #[cfg(target_family = "wasm")]
    Host(HostBytes),
}

impl Payload {
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        match self {
            Payload::Bytes(bytes) => bytes.len(),
            #[cfg(target_family = "wasm")]
            Payload::Host(bytes) => bytes.len(),
        }
    }

    /// The bytes in wasm memory: a JS array's, copied.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        match self {
            Payload::Bytes(bytes) => bytes,
            #[cfg(target_family = "wasm")]
            Payload::Host(bytes) => bytes.to_vec(),
        }
    }
}

impl From<Vec<u8>> for Payload {
    fn from(bytes: Vec<u8>) -> Self {
        Payload::Bytes(bytes)
    }
}

/// The smallest array that travels as a payload.
pub(crate) const PAYLOAD_MIN: usize = 4096;

thread_local! {
    /// While `encode` runs: the payloads so far.
    static ENCODING: RefCell<Option<Vec<Payload>>> = const { RefCell::new(None) };
    /// While `decode` runs: the payloads not taken yet.
    static DECODING: RefCell<Option<Vec<Option<Payload>>>> = const { RefCell::new(None) };
}

/// Runs `encode`, which serializes a message, with a session open: returns
/// its result and the payloads the message refers to.
pub fn encode<R>(encode: impl FnOnce() -> R) -> (R, Vec<Payload>) {
    let outer = ENCODING.with(|session| session.borrow_mut().replace(Vec::new()));
    let result = encode();
    let payloads = ENCODING.with(|session| std::mem::replace(&mut *session.borrow_mut(), outer));
    (result, payloads.unwrap_or_default())
}

/// Runs `decode`, which deserializes a message that `encode` made, with its
/// payloads.
pub fn decode<R>(payloads: Vec<Payload>, decode: impl FnOnce() -> R) -> R {
    let payloads = payloads.into_iter().map(Some).collect();
    let outer = DECODING.with(|session| session.borrow_mut().replace(payloads));
    let result = decode();
    DECODING.with(|session| *session.borrow_mut() = outer);
    result
}

/// How an array is written: in the message, or as the payload at an index.
#[derive(Serialize)]
enum Written<'a> {
    Inline(#[serde(with = "inline")] &'a [u8]),
    Payload(u32),
}

#[derive(Deserialize)]
enum Read {
    Inline(#[serde(with = "inline")] Vec<u8>),
    Payload(u32),
}

/// serde bytes, both ways.
mod inline {
    use serde::{Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(bytes: &&[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bytes")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
                Ok(v.to_vec())
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
                Ok(v)
            }
        }
        deserializer.deserialize_byte_buf(Visitor)
    }
}

fn write<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    let index = if bytes.len() >= PAYLOAD_MIN {
        ENCODING.with(|session| {
            session.borrow_mut().as_mut().map(|payloads| {
                payloads.push(Payload::Bytes(bytes.to_vec()));
                payloads.len() - 1
            })
        })
    } else {
        None
    };
    match index {
        Some(index) => {
            let index = u32::try_from(index).map_err(|_| serde::ser::Error::custom("too many payloads"))?;
            Written::Payload(index).serialize(serializer)
        }
        None => Written::Inline(bytes).serialize(serializer),
    }
}

fn read_payload<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Payload, D::Error> {
    match Read::deserialize(deserializer)? {
        Read::Inline(bytes) => Ok(Payload::Bytes(bytes)),
        Read::Payload(index) => DECODING
            .with(|session| session.borrow_mut().as_mut().and_then(|payloads| payloads.get_mut(index as usize)?.take()))
            .ok_or_else(|| D::Error::custom(format!("no payload {index}"))),
    }
}

fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    read_payload(deserializer).map(Payload::into_vec)
}

/// A `Vec<u8>`.
pub mod bytes {
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        super::write(bytes, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        super::read(deserializer)
    }
}

/// An `Arc<Vec<u8>>` (encoded tile data and images).
pub mod shared_bytes {
    use std::sync::Arc;

    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Arc<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error> {
        super::write(bytes, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<Vec<u8>>, D::Error> {
        super::read(deserializer).map(Arc::new)
    }

    /// An `Option<Arc<Vec<u8>>>`.
    pub mod option {
        use std::sync::Arc;

        use serde::{Deserialize, Deserializer, Serialize, Serializer};

        #[derive(Serialize, Deserialize)]
        struct Bytes(#[serde(with = "super")] Arc<Vec<u8>>);

        pub fn serialize<S: Serializer>(bytes: &Option<Arc<Vec<u8>>>, serializer: S) -> Result<S::Ok, S::Error> {
            bytes.clone().map(Bytes).serialize(serializer)
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Arc<Vec<u8>>>, D::Error> {
            Ok(Option::<Bytes>::deserialize(deserializer)?.map(|bytes| bytes.0))
        }
    }
}

/// A `Vec` of plain values, as their bytes.
pub mod pod {
    use serde::de::Error as _;
    use serde::{Deserializer, Serializer};

    use super::Payload;

    pub fn serialize<T: bytemuck::Pod, S: Serializer>(values: &[T], serializer: S) -> Result<S::Ok, S::Error> {
        super::write(bytemuck::cast_slice(values), serializer)
    }

    pub fn deserialize<'de, T: bytemuck::Pod, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<T>, D::Error> {
        Ok(match deserialize_payload::<T, D>(deserializer)? {
            Payload::Bytes(bytes) => bytemuck::pod_collect_to_vec(&bytes),
            #[cfg(target_family = "wasm")]
            Payload::Host(bytes) => bytes.to_pod_vec(),
        })
    }

    /// The values' bytes as they arrived, a whole number of `T`s.
    pub fn deserialize_payload<'de, T: bytemuck::Pod, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Payload, D::Error> {
        let bytes = super::read_payload(deserializer)?;
        if !bytes.len().is_multiple_of(size_of::<T>()) {
            let size = size_of::<T>();
            return Err(D::Error::custom(format!(
                "{} bytes are not a whole number of {size}-byte values",
                bytes.len()
            )));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Message {
        #[serde(with = "bytes")]
        small: Vec<u8>,
        #[serde(with = "bytes")]
        large: Vec<u8>,
        #[serde(with = "pod")]
        values: Vec<[u16; 2]>,
    }

    fn message() -> Message {
        Message { small: vec![1, 2, 3], large: vec![7; PAYLOAD_MIN], values: vec![[1, 2]; PAYLOAD_MIN] }
    }

    /// In a session, the large arrays travel beside the message; outside
    /// one, everything is in it.
    #[test]
    fn large_arrays_travel_beside_the_message() {
        let (bytes, payloads) = encode(|| postcard::to_allocvec(&message()).unwrap());
        assert_eq!(payloads.len(), 2);
        assert!(bytes.len() < 32);
        let decoded: Message = decode(payloads, || postcard::from_bytes(&bytes).unwrap());
        assert_eq!(decoded, message());

        let inline = postcard::to_allocvec(&message()).unwrap();
        assert!(inline.len() > 5 * PAYLOAD_MIN);
        assert_eq!(postcard::from_bytes::<Message>(&inline).unwrap(), message());
    }

    /// A message that refers to a payload it doesn't have is malformed.
    #[test]
    fn missing_payloads_are_errors() {
        let (bytes, _) = encode(|| postcard::to_allocvec(&message()).unwrap());
        assert!(decode(Vec::new(), || postcard::from_bytes::<Message>(&bytes)).is_err());
    }
}
