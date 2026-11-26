// Port-only: the frames that cross between the map side and a worker
// instance (mod.rs), and how they are encoded.
//
// Every frame is one job's: a `Run` to a worker, a `Reply` back. It starts
// with the job's id as a fixed header, read without the shared tables, and
// has its body after it: a worker that can't decode a body replies `Failed`
// with the header's id, and the map side drops a stale reply after reading
// only its id.

use std::time::Duration;

use mln_style::tile_id::CanonicalTileID;
use mln_tile_id::OverscaledTileID;
use mln_wire::payload::Payload;
use mln_wire::{Context, Reader, Wire, Writer};

use crate::tile::job::{JobId, JobTimes, SourceKey, TileKey};

/// A frame as it crosses between instances: its bytes, and the byte arrays
/// that travel beside them (`mln_wire::payload`), which a host can transfer
/// rather than copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub payloads: Vec<Payload>,
}

impl From<Vec<u8>> for Encoded {
    /// A frame without payloads.
    fn from(bytes: Vec<u8>) -> Self {
        Encoded { bytes, payloads: Vec::new() }
    }
}

/// The size of a `JobId` in a header.
const JOB_ID_SIZE: usize = 8 + 1 + 2 + 1 + 4 + 4 + 8;

/// Appends a job frame's header: `id`.
pub(crate) fn write_job_header(bytes: &mut Vec<u8>, id: &JobId) {
    let JobId { tile: TileKey { source: SourceKey(source), id: tile }, generation } = *id;
    bytes.extend_from_slice(&source.to_le_bytes());
    bytes.push(tile.overscaled_z);
    bytes.extend_from_slice(&tile.wrap.to_le_bytes());
    bytes.push(tile.canonical.z);
    bytes.extend_from_slice(&tile.canonical.x.to_le_bytes());
    bytes.extend_from_slice(&tile.canonical.y.to_le_bytes());
    bytes.extend_from_slice(&generation.to_le_bytes());
}

/// Reads a job frame's header: its id, and the bytes after it.
pub(crate) fn read_job_id(bytes: &[u8]) -> Option<(JobId, &[u8])> {
    let (header, rest) = bytes.split_at_checked(JOB_ID_SIZE)?;
    let (source, header) = header.split_first_chunk::<8>()?;
    let (&[overscaled_z], header) = header.split_first_chunk::<1>()?;
    let (wrap, header) = header.split_first_chunk::<2>()?;
    let (&[z], header) = header.split_first_chunk::<1>()?;
    let (x, header) = header.split_first_chunk::<4>()?;
    let (y, header) = header.split_first_chunk::<4>()?;
    let (generation, _) = header.split_first_chunk::<8>()?;
    let canonical = CanonicalTileID { z, x: u32::from_le_bytes(*x), y: u32::from_le_bytes(*y) };
    let tile = OverscaledTileID { overscaled_z, wrap: i16::from_le_bytes(*wrap), canonical };
    let tile = TileKey { source: SourceKey(u64::from_le_bytes(*source)), id: tile };
    Some((JobId { tile, generation: u64::from_le_bytes(*generation) }, rest))
}

/// Appends a reply's header: the job's id and how long its stages took.
pub(crate) fn write_reply_header(bytes: &mut Vec<u8>, id: &JobId, times: &JobTimes) {
    write_job_header(bytes, id);
    let micros = |time: Duration| u32::try_from(time.as_micros()).unwrap_or(u32::MAX);
    bytes.extend_from_slice(&micros(times.discover).to_le_bytes());
    bytes.extend_from_slice(&micros(times.build).to_le_bytes());
}

/// Reads a reply's header: the job's id, its times, and the body after
/// them.
pub(crate) fn read_reply_header(bytes: &[u8]) -> Option<(JobId, JobTimes, &[u8])> {
    let (id, rest) = read_job_id(bytes)?;
    let (discover, rest) = rest.split_first_chunk::<4>()?;
    let (build, rest) = rest.split_first_chunk::<4>()?;
    let time = |micros: &[u8; 4]| Duration::from_micros(u64::from(u32::from_le_bytes(*micros)));
    Some((id, JobTimes { discover: time(discover), build: time(build) }, rest))
}

/// Appends `value`, encoded in `context`.
pub(crate) fn append<T: Wire>(bytes: &mut Vec<u8>, value: &T, mut context: impl Context) -> Result<(), String> {
    let mut w = Writer::from_vec(std::mem::take(bytes)).with_context(&mut context);
    value.write(&mut w);
    *bytes = w.finish().map_err(|e| format!("can't encode: {e}"))?;
    Ok(())
}

/// Decodes a value at the start of `bytes`; returns it and the bytes after
/// it.
pub(crate) fn take<T: Wire>(bytes: &[u8]) -> Result<(T, &[u8]), String> {
    let mut r = Reader::new(bytes);
    match T::read(&mut r) {
        Some(value) => Ok((value, r.rest())),
        None => Err(format!("malformed: {}", r.error())),
    }
}

/// Decodes a value that fills `bytes`, in `context`.
pub(crate) fn decode<T: Wire>(bytes: &[u8], mut context: impl Context) -> Result<T, String> {
    let mut r = Reader::new(bytes).with_context(&mut context);
    match T::read(&mut r) {
        Some(_) if !r.rest().is_empty() => Err(format!("malformed: {} bytes after the end", r.rest().len())),
        Some(value) => Ok(value),
        None => Err(format!("malformed: {}", r.error())),
    }
}
