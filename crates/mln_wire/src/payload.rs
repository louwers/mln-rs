//! Port-only: byte arrays that travel beside a message.
//!
//! In a message written with [`Payloads`] in its context, an array of at
//! least `PAYLOAD_MIN` bytes is not written into the message: it is moved to
//! the payloads, and the message refers to it by index. Reading the message
//! with its payloads in the context gives them back to the arrays that refer
//! to them, as they are. Fields opt in with `#[wire(with = "...")]`: `bytes`
//! for a `Vec<u8>`, `shared_bytes` for an `Arc<Vec<u8>>` and `pod` for a
//! `Vec` of plain values (vertices, indices). Without payloads in the
//! context the arrays are written in the message.
//!
//! On wasm a host can hand over its payloads as the JS arrays they arrived
//! in (`Payload::Host`). Each field copies its array into wasm memory, but
//! for `pod::read_payload`, whose caller can keep it in JS.

#[cfg(target_family = "wasm")]
use crate::host_bytes::HostBytes;

/// A payload as a message is read with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Bytes(Vec<u8>),
    /// A JS array, outside wasm memory.
    #[cfg(target_family = "wasm")]
    Host(HostBytes),
}

impl Payload {
    /// The number of bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Payload::Bytes(bytes) => bytes.len(),
            #[cfg(target_family = "wasm")]
            Payload::Host(bytes) => bytes.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

/// The payloads of one message, as a part of the context it is written or
/// read in: those written so far, or those not read yet.
#[derive(Debug, Default)]
pub struct Payloads(Vec<Option<Payload>>);

impl Payloads {
    /// The payloads of a message to write.
    #[must_use]
    pub fn new() -> Self {
        Payloads::default()
    }

    /// The payloads written.
    #[must_use]
    pub fn into_vec(self) -> Vec<Payload> {
        self.0.into_iter().flatten().collect()
    }
}

/// The payloads of a message to read.
impl From<Vec<Payload>> for Payloads {
    fn from(payloads: Vec<Payload>) -> Self {
        Payloads(payloads.into_iter().map(Some).collect())
    }
}

impl crate::Context for Payloads {
    fn provide<'a>(&'a mut self, request: &mut crate::Request<'a>) {
        request.give_mut(self);
    }
}

/// Writes an array: 0 and the bytes in the message, or 1 and the index of
/// the payload it moved to.
fn write(bytes: &[u8], w: &mut crate::Writer) {
    let index = if bytes.len() >= PAYLOAD_MIN {
        w.context_mut::<Payloads>().map(|Payloads(payloads)| {
            payloads.push(Some(Payload::Bytes(bytes.to_vec())));
            payloads.len() - 1
        })
    } else {
        None
    };
    if let Some(index) = index {
        w.push(1);
        crate::write_varint(w, index as u64);
    } else {
        w.push(0);
        crate::write_varint(w, bytes.len() as u64);
        w.extend_from_slice(bytes);
    }
}

/// Reads what `write` wrote.
fn read_payload(r: &mut crate::Reader<'_>) -> Option<Payload> {
    match r.byte()? {
        0 => {
            let len = r.length()?;
            Some(Payload::Bytes(r.take(len)?.to_vec()))
        }
        1 => {
            let index = r.varint()?;
            let payload =
                usize::try_from(index).ok().and_then(|index| r.context_mut::<Payloads>()?.0.get_mut(index)?.take());
            payload.or_else(|| r.fail("a payload the message doesn't have"))
        }
        _ => r.fail_tag(),
    }
}

fn read(r: &mut crate::Reader<'_>) -> Option<Vec<u8>> {
    read_payload(r).map(Payload::into_vec)
}

/// A `Vec<u8>`.
pub mod bytes {
    pub fn write(bytes: &[u8], w: &mut crate::Writer) {
        super::write(bytes, w);
    }

    pub fn read(r: &mut crate::Reader<'_>) -> Option<Vec<u8>> {
        super::read(r)
    }
}

/// An `Arc<Vec<u8>>` (encoded tile data and images).
pub mod shared_bytes {
    use std::sync::Arc;

    pub fn write(bytes: &Arc<Vec<u8>>, w: &mut crate::Writer) {
        super::write(bytes, w);
    }

    pub fn read(r: &mut crate::Reader<'_>) -> Option<Arc<Vec<u8>>> {
        super::read(r).map(Arc::new)
    }

    /// An `Option<Arc<Vec<u8>>>`.
    pub mod option {
        use std::sync::Arc;

        pub fn write(bytes: &Option<Arc<Vec<u8>>>, w: &mut crate::Writer) {
            match bytes {
                None => w.push(0),
                Some(bytes) => {
                    w.push(1);
                    super::write(bytes, w);
                }
            }
        }

        pub fn read(r: &mut crate::Reader<'_>) -> Option<Option<Arc<Vec<u8>>>> {
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

    pub fn write<T: bytemuck::Pod>(values: &[T], w: &mut crate::Writer) {
        super::write(bytemuck::cast_slice(values), w);
    }

    pub fn read<T: bytemuck::Pod>(r: &mut crate::Reader<'_>) -> Option<Vec<T>> {
        Some(match read_payload::<T>(r)? {
            Payload::Bytes(bytes) => bytemuck::pod_collect_to_vec(&bytes),
            #[cfg(target_family = "wasm")]
            Payload::Host(bytes) => bytes.to_pod_vec(),
        })
    }

    /// The values' bytes as they arrived, a whole number of `T`s.
    pub fn read_payload<T: bytemuck::Pod>(r: &mut crate::Reader<'_>) -> Option<Payload> {
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

    #[derive(Debug, PartialEq, crate::Wire)]
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

    /// With payloads in the context, the large arrays travel beside the
    /// message; without, everything is in it.
    #[test]
    fn large_arrays_travel_beside_the_message() {
        let mut payloads = Payloads::new();
        let bytes = crate::to_vec_in(&message(), &mut payloads).unwrap();
        let payloads = payloads.into_vec();
        assert_eq!(payloads.len(), 2);
        assert!(bytes.len() < 32);
        let decoded: Message = crate::from_slice_in(&bytes, Payloads::from(payloads)).unwrap();
        assert_eq!(decoded, message());

        let inline = crate::to_vec(&message()).unwrap();
        assert!(inline.len() > 5 * PAYLOAD_MIN);
        assert_eq!(crate::from_slice::<Message>(&inline).unwrap(), message());
    }

    /// A message that refers to a payload it doesn't have is malformed.
    #[test]
    fn missing_payloads_are_errors() {
        let bytes = crate::to_vec_in(&message(), Payloads::new()).unwrap();
        assert!(crate::from_slice_in::<Message>(&bytes, Payloads::new()).is_err());
    }
}
