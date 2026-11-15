// Port-only: what goes around the messages between the map side and a worker
// instance (mod.rs), and how both are encoded.
//
// A request is a `Request`, then the message it carries, if any; a reply is a
// `ReplyHead`, then the worker's message. Each is encoded with mln_wire, one
// after the other, so that the side that gets them reads the envelope even
// when the message can't be read: the instance tells the tile which message
// failed, and the map side which tile a reply it can't read was for.

use mln_util::serialization::payload::Payload;
use mln_wire::{Reader, Wire, Writer};

use crate::tile::tile_worker_transport::WorkerParameters;

/// A reply as it crosses between instances: its bytes, and the byte arrays
/// that travel beside them (`mln_util::serialization::payload`), which a host
/// can transfer rather than copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub payloads: Vec<Payload>,
}

impl From<Vec<u8>> for Encoded {
    /// A reply without payloads.
    fn from(bytes: Vec<u8>) -> Self {
        Encoded { bytes, payloads: Vec::new() }
    }
}

/// What the map side sends a worker instance, before the message it
/// carries.
#[derive(Wire)]
pub(crate) struct Request {
    /// The keys of the shared values that are gone on the map side
    /// (`shared::Tables::forget_dead`).
    pub(crate) forget: Vec<(String, Vec<u64>)>,
    pub(crate) action: Action,
}

#[derive(Wire)]
pub(crate) enum Action {
    /// Creates a worker for the geometry tile `tile`.
    Create { tile: u64, parameters: WorkerParameters },
    /// Creates a worker for the raster tile `tile`.
    CreateRaster { tile: u64 },
    /// Creates a worker for the raster-dem tile `tile`.
    CreateRasterDEM { tile: u64 },
    /// A message for the worker of `tile`, of its type, which follows.
    Message { tile: u64, about: About },
    /// The worker's destructor: `~GeometryTileWorker()` after the tile set
    /// its `obsolete` flag, or the raster or raster-dem tile's worker.
    Remove { tile: u64 },
}

/// What a message is, for an error about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Wire)]
pub(crate) struct About {
    /// The message's correlation id, or 0 for the answer to a dependency
    /// request, which has none (the tile reports an error without one
    /// without clearing its pending state).
    pub(crate) correlation_id: u64,
    pub(crate) kind: MessageKind,
}

/// The messages whose failure loses what the worker had (host.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Wire)]
pub(crate) enum MessageKind {
    SetData,
    SetLayers,
    /// Any other message, which changes neither the data nor the layers.
    Other,
}

/// What a worker instance sends back before the message of `tile`'s worker
/// for its tile.
#[derive(Wire)]
pub(crate) struct ReplyHead {
    pub(crate) tile: u64,
    /// The message's correlation id, for an error about it.
    pub(crate) correlation_id: u64,
}

/// Appends `value`, encoded.
pub(crate) fn append<T: Wire>(bytes: &mut Vec<u8>, value: &T) -> Result<(), String> {
    append_with(bytes, |w| value.write(w))
}

/// Appends what `write` writes.
fn append_with(bytes: &mut Vec<u8>, write: impl FnOnce(&mut Writer)) -> Result<(), String> {
    let mut w = Writer::from_vec(std::mem::take(bytes));
    write(&mut w);
    *bytes = w.finish().map_err(|e| format!("can't encode: {e}"))?;
    Ok(())
}

/// Decodes a value at the start of `bytes`; returns it and the bytes after
/// it.
pub(crate) fn take<T: Wire>(bytes: &[u8]) -> Result<(T, &[u8]), String> {
    take_with(bytes, T::read)
}

/// `take` with `read` for the value.
fn take_with<T>(bytes: &[u8], read: impl FnOnce(&mut Reader<'_>) -> Option<T>) -> Result<(T, &[u8]), String> {
    let mut r = Reader::new(bytes);
    match read(&mut r) {
        Some(value) => Ok((value, r.rest())),
        None => Err(format!("malformed: {}", r.error())),
    }
}

/// Decodes a value that fills `bytes`.
pub(crate) fn decode<T: Wire>(bytes: &[u8]) -> Result<T, String> {
    match take(bytes)? {
        (value, []) => Ok(value),
        (_, rest) => Err(format!("malformed: {} bytes after the end", rest.len())),
    }
}
