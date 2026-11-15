// Port-only: a worker instance's side of the worker codec (mod.rs): decodes
// what the map side sends the instance's workers and encodes what they send
// back.

use mln_style::serialization::{SharedExpression, SharedLayer, SharedLayerProperties};
use mln_util::error::Error;
use mln_util::serialization::{payload, shared};
use mln_wire::Wire;

use super::envelope::{self, About, Action, Encoded, ReplyHead, Request};
use crate::tile::geometry_tile_worker::TileMessage;
use crate::tile::raster_dem_tile::RasterDEMTileMessage;
use crate::tile::raster_tile::RasterTileMessage;
use crate::tile::tile_worker_transport::WorkerParameters;

/// What a worker instance gets from the map side.
pub(crate) enum WorkerAction {
    /// Create the worker of `tile`.
    Create { tile: u64, parameters: WorkerParameters },
    /// Create the `RasterTileWorker` of `tile`.
    CreateRaster { tile: u64 },
    /// Create the `RasterDEMTileWorker` of `tile`.
    CreateRasterDEM { tile: u64 },
    /// A message for the worker of `tile`, still encoded: the host decodes
    /// it as its worker type's (`WorkerCodec::decode_message`) and reports
    /// it to the tile if it can't.
    Message { tile: u64, about: About, message: Vec<u8> },
    /// Drop the worker of `tile`.
    Remove { tile: u64 },
}

/// A worker instance's side.
pub(crate) struct WorkerCodec {
    /// The values the map side defined, by key.
    tables: shared::Tables,
}

impl Default for WorkerCodec {
    fn default() -> Self {
        let tables = shared::Tables::new()
            .copies::<SharedLayer>()
            .copies::<SharedLayerProperties>()
            .copies::<SharedExpression>()
            .copies::<mln_tile::TileBytes>()
            .copies::<mln_tile::TileFeatures>();
        WorkerCodec { tables }
    }
}

impl WorkerCodec {
    pub(crate) fn new() -> Self {
        WorkerCodec::default()
    }

    /// Decodes a request, and forgets what it forgets. Fails only for bytes
    /// that aren't a request.
    pub(crate) fn decode(&mut self, bytes: &[u8]) -> Result<WorkerAction, String> {
        let (request, message): (Request, &[u8]) = self.tables.open(|| envelope::take(bytes))?;
        self.tables.forget(&request.forget);
        Ok(match request.action {
            Action::Create { tile, parameters } => WorkerAction::Create { tile, parameters },
            Action::CreateRaster { tile } => WorkerAction::CreateRaster { tile },
            Action::CreateRasterDEM { tile } => WorkerAction::CreateRasterDEM { tile },
            Action::Message { tile, about } => WorkerAction::Message { tile, about, message: message.to_vec() },
            Action::Remove { tile } => WorkerAction::Remove { tile },
        })
    }

    /// Decodes the message of a `WorkerAction::Message` as an `M`.
    pub(crate) fn decode_message<M: Wire>(&mut self, message: &[u8]) -> Result<M, String> {
        self.tables.open(|| envelope::decode(message))
    }

    /// Encodes what the worker of `tile` sends its tile, its arrays as
    /// payloads. What can't cross becomes an error for the tile.
    pub(crate) fn encode(&mut self, tile: u64, message: &TileMessage) -> Encoded {
        let correlation_id = match message {
            TileMessage::Layout { correlation_id, .. } | TileMessage::Error { correlation_id, .. } => *correlation_id,
            TileMessage::GetDependencies { .. } => 0,
        };
        self.encode_reply(tile, correlation_id, message, |error| TileMessage::Error {
            error: Error::native(error),
            correlation_id,
        })
    }

    /// Encodes what the `RasterTileWorker` of `tile` sends its tile.
    pub(crate) fn encode_raster(&mut self, tile: u64, message: &RasterTileMessage) -> Encoded {
        let (&RasterTileMessage::Parsed { correlation_id, .. } | &RasterTileMessage::Error { correlation_id, .. }) =
            message;
        self.encode_reply(tile, correlation_id, message, |error| RasterTileMessage::Error {
            error: Error::native(error),
            correlation_id,
        })
    }

    /// Encodes what the `RasterDEMTileWorker` of `tile` sends its tile.
    pub(crate) fn encode_raster_dem(&mut self, tile: u64, message: &RasterDEMTileMessage) -> Encoded {
        let (&RasterDEMTileMessage::Parsed { correlation_id, .. }
        | &RasterDEMTileMessage::Error { correlation_id, .. }) = message;
        self.encode_reply(tile, correlation_id, message, |error| RasterDEMTileMessage::Error {
            error: Error::native(error),
            correlation_id,
        })
    }

    /// A reply: its head, and then `message`, or the `error` of what failed
    /// to encode.
    fn encode_reply<M: Wire>(
        &mut self,
        tile: u64,
        correlation_id: u64,
        message: &M,
        error: impl FnOnce(String) -> M,
    ) -> Encoded {
        let head = ReplyHead { tile, correlation_id };
        let encode = |message: &M| {
            let mut bytes = Vec::new();
            envelope::append(&mut bytes, &head)?;
            envelope::append(&mut bytes, message)?;
            Ok::<_, String>(bytes)
        };
        let (bytes, payloads) = payload::encode(|| {
            let encoded = self.tables.open(|| encode(message));
            encoded.or_else(|failure| self.tables.open(|| encode(&error(failure))))
        });
        Encoded { bytes: bytes.unwrap_or_default(), payloads }
    }
}
