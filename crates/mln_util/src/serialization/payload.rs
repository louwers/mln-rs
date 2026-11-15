//! Port-only: byte arrays that travel beside a message (serialization/mod.rs).
//!
//! In a message encoded inside `encode`, an array of at least `PAYLOAD_MIN`
//! bytes is not written into the message: it is moved to the session's
//! payloads, and the message refers to it by index. `decode` gives the
//! payloads back to the arrays that refer to them, as they are. Fields opt
//! in with `#[wire(with = "...")]`: `bytes` for a `Vec<u8>`, `shared_bytes`
//! for an `Arc<Vec<u8>>` and `pod` for a `Vec` of plain values (vertices,
//! indices). Outside a session the arrays are written in the message.
//!
//! On wasm a host can hand `decode` its payloads as the JS arrays they
//! arrived in (`Payload::Host`). Each field copies its array into wasm
//! memory, but for `pod::read_payload`, whose caller can keep it in JS.

use std::cell::RefCell;

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

/// Writes an array: 0 and the bytes in the message, or 1 and the index of
/// the payload it moved to.
fn write(bytes: &[u8], w: &mut mln_wire::Writer) {
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
    if let Some(index) = index {
        w.push(1);
        mln_wire::write_varint(w, index as u64);
    } else {
        w.push(0);
        mln_wire::write_varint(w, bytes.len() as u64);
        w.extend_from_slice(bytes);
    }
}

/// Reads what `write` wrote.
fn read_payload(r: &mut mln_wire::Reader<'_>) -> Option<Payload> {
    match r.byte()? {
        0 => {
            let len = r.length()?;
            Some(Payload::Bytes(r.take(len)?.to_vec()))
        }
        1 => {
            let index = r.varint()?;
            let payload = DECODING.with(|session| {
                let mut session = session.borrow_mut();
                session.as_mut()?.get_mut(usize::try_from(index).ok()?)?.take()
            });
            payload.or_else(|| r.fail("a payload the message doesn't have"))
        }
        _ => r.fail_tag(),
    }
}

fn read(r: &mut mln_wire::Reader<'_>) -> Option<Vec<u8>> {
    read_payload(r).map(Payload::into_vec)
}

/// A `Vec<u8>`.
pub mod bytes {
    pub fn write(bytes: &[u8], w: &mut mln_wire::Writer) {
        super::write(bytes, w);
    }

    pub fn read(r: &mut mln_wire::Reader<'_>) -> Option<Vec<u8>> {
        super::read(r)
    }
}

/// An `Arc<Vec<u8>>` (encoded tile data and images).
pub mod shared_bytes {
    use std::sync::Arc;

    pub fn write(bytes: &Arc<Vec<u8>>, w: &mut mln_wire::Writer) {
        super::write(bytes, w);
    }

    pub fn read(r: &mut mln_wire::Reader<'_>) -> Option<Arc<Vec<u8>>> {
        super::read(r).map(Arc::new)
    }

    /// An `Option<Arc<Vec<u8>>>`.
    pub mod option {
        use std::sync::Arc;

        pub fn write(bytes: &Option<Arc<Vec<u8>>>, w: &mut mln_wire::Writer) {
            match bytes {
                None => w.push(0),
                Some(bytes) => {
                    w.push(1);
                    super::write(bytes, w);
                }
            }
        }

        pub fn read(r: &mut mln_wire::Reader<'_>) -> Option<Option<Arc<Vec<u8>>>> {
            match r.byte()? {
                0 => Some(None),
                1 => super::read(r).map(Some),
                _ => r.fail("invalid option"),
            }
        }
    }
}

/// A `Vec` of plain values, as their bytes.
pub mod pod {
    use super::Payload;

    pub fn write<T: bytemuck::Pod>(values: &[T], w: &mut mln_wire::Writer) {
        super::write(bytemuck::cast_slice(values), w);
    }

    pub fn read<T: bytemuck::Pod>(r: &mut mln_wire::Reader<'_>) -> Option<Vec<T>> {
        Some(match read_payload::<T>(r)? {
            Payload::Bytes(bytes) => bytemuck::pod_collect_to_vec(&bytes),
            #[cfg(target_family = "wasm")]
            Payload::Host(bytes) => bytes.to_pod_vec(),
        })
    }

    /// The values' bytes as they arrived, a whole number of `T`s.
    pub fn read_payload<T: bytemuck::Pod>(r: &mut mln_wire::Reader<'_>) -> Option<Payload> {
        let bytes = super::read_payload(r)?;
        if !bytes.len().is_multiple_of(size_of::<T>()) {
            return r.fail("bytes that are not a whole number of values");
        }
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, mln_wire::Wire)]
    struct Message {
        #[wire(with = "bytes")]
        small: Vec<u8>,
        #[wire(with = "bytes")]
        large: Vec<u8>,
        #[wire(with = "pod")]
        values: Vec<[u16; 2]>,
    }

    fn message() -> Message {
        Message { small: vec![1, 2, 3], large: vec![7; PAYLOAD_MIN], values: vec![[1, 2]; PAYLOAD_MIN] }
    }

    /// In a session, the large arrays travel beside the message; outside
    /// one, everything is in it.
    #[test]
    fn large_arrays_travel_beside_the_message() {
        let (bytes, payloads) = encode(|| mln_wire::to_vec(&message()).unwrap());
        assert_eq!(payloads.len(), 2);
        assert!(bytes.len() < 32);
        let decoded: Message = decode(payloads, || mln_wire::from_slice(&bytes).unwrap());
        assert_eq!(decoded, message());

        let inline = mln_wire::to_vec(&message()).unwrap();
        assert!(inline.len() > 5 * PAYLOAD_MIN);
        assert_eq!(mln_wire::from_slice::<Message>(&inline).unwrap(), message());
    }

    /// A message that refers to a payload it doesn't have is malformed.
    #[test]
    fn missing_payloads_are_errors() {
        let (bytes, _) = encode(|| mln_wire::to_vec(&message()).unwrap());
        assert!(decode(Vec::new(), || mln_wire::from_slice::<Message>(&bytes)).is_err());
    }
}
