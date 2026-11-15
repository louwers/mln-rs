// Port-only: the map side of the worker codec (mod.rs), for one worker
// instance: encodes the jobs for that instance and decodes their replies,
// and does the same for the messages of raster and raster-dem tiles.

use mln_style::serialization::{SharedExpression, SharedLayer, SharedLayerProperties};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::serialization::payload::Payload;
use mln_util::serialization::{payload, shared};
use mln_wire::Wire;

use super::envelope::{self, Action, Encoded, RASTER, ReplyHead};
use super::fields::{SharedAvailableImages, SharedSourceStyle};
use crate::tile::job::{JobResult, Run};
use crate::tile::raster_dem_tile::RasterDEMTileMessage;
use crate::tile::raster_tile::RasterTileMessage;

/// The map side of one worker instance's jobs.
pub(crate) struct MapCodec {
    /// The keys of what the instance has of this side's: source styles,
    /// layers, their properties and expressions, and sets of images.
    tables: shared::Tables,
}

impl Default for MapCodec {
    fn default() -> Self {
        MapCodec {
            tables: shared::Tables::new()
                .origin::<SharedSourceStyle>()
                .origin::<SharedAvailableImages>()
                .origin::<SharedLayer>()
                .origin::<SharedLayerProperties>()
                .origin::<SharedExpression>(),
        }
    }
}

impl MapCodec {
    /// The frame of `run`: its header, then the keys of the shared values
    /// that are gone on this side, then the job. Fails for what can't cross,
    /// which then defines nothing.
    pub(crate) fn encode_run(&mut self, run: &Run) -> Result<Vec<u8>, String> {
        let job = self.tables.open(|| {
            let mut bytes = Vec::new();
            envelope::append(&mut bytes, &run.job)?;
            Ok::<_, String>(bytes)
        })?;
        // After the job, which keeps alive what it defines.
        let forget = self.tables.forget_dead();
        let mut bytes = Vec::new();
        envelope::write_job_header(&mut bytes, &run.id);
        envelope::append(&mut bytes, &forget)?;
        bytes.extend_from_slice(&job);
        Ok(bytes)
    }

    /// The result in a reply's body. A body this side can't decode becomes
    /// `Failed`.
    pub(crate) fn decode_result(&mut self, body: Encoded) -> JobResult {
        let Encoded { bytes, payloads } = body;
        payload::decode(payloads, || self.tables.open(|| envelope::decode::<JobResult>(&bytes)))
            .unwrap_or_else(|error| JobResult::Failed(Error::native(format!("a tile worker's reply: {error}"))))
    }
}

/// A raster or raster-dem tile's frame: `action`, and then `message` if
/// there is one.
pub(crate) fn raster_request<M: Wire>(action: &Action, message: Option<&M>) -> Vec<u8> {
    let mut bytes = vec![RASTER];
    let encoded = envelope::append(&mut bytes, action).and_then(|()| match message {
        Some(message) => envelope::append(&mut bytes, message),
        None => Ok(()),
    });
    if let Err(error) = encoded {
        Log::error(Event::General, format!("Can't encode a tile worker request: {error}"));
    }
    bytes
}

/// A raster or raster-dem tile's reply, decoded as its worker type's.
pub(crate) enum RasterReply {
    Raster(RasterTileMessage),
    RasterDEM(RasterDEMTileMessage),
}

/// Decodes a raster or raster-dem tile's reply, after its kind: the
/// tile's `T`, which `route` gives with whether the tile is a raster-dem
/// tile, and the message, as that tile's type's. `None` for a tile `route`
/// doesn't know. A message this side can't decode becomes an error for the
/// tile. Fails only for bytes that aren't a reply.
pub(crate) fn decode_raster_reply<T>(
    bytes: &[u8],
    payloads: Vec<Payload>,
    route: impl FnOnce(u64) -> Option<(bool, T)>,
) -> Result<Option<(T, RasterReply)>, String> {
    payload::decode(payloads, || {
        let (head, message): (ReplyHead, &[u8]) = envelope::take(bytes)?;
        let Some((dem, tile)) = route(head.tile) else {
            return Ok(None);
        };
        let error = |message: String| Error::native(message);
        let correlation_id = head.correlation_id;
        let reply = if dem {
            RasterReply::RasterDEM(
                envelope::decode(message)
                    .unwrap_or_else(|message| RasterDEMTileMessage::Error { error: error(message), correlation_id }),
            )
        } else {
            RasterReply::Raster(
                envelope::decode(message)
                    .unwrap_or_else(|message| RasterTileMessage::Error { error: error(message), correlation_id }),
            )
        };
        Ok(Some((tile, reply)))
    })
}
