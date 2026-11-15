// Port-only: the map side of the worker codec (mod.rs), for one worker
// instance: encodes what the tiles send their workers in that instance and
// decodes what the workers send back.

use std::sync::Arc;

use mln_style::layer_properties::LayerProperties;
use mln_style::serialization::{SharedExpression, SharedLayer, SharedLayerProperties};
use mln_tile::TileData;
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::serialization::{payload, shared};
use mln_wire::Wire;

use super::envelope::{self, About, Action, Encoded, MessageKind, ReplyHead, Request};
use crate::tile::geometry_tile_worker::{TileMessage, WorkerMessage};
use crate::tile::raster_dem_tile::{RasterDEMTileMessage, RasterDEMWorkerMessage};
use crate::tile::raster_tile::{RasterTileMessage, RasterWorkerMessage};
use crate::tile::tile_worker_transport::WorkerParameters;

/// A worker's message for its tile, decoded: the messages of the tile's
/// worker type.
pub(crate) enum Reply {
    Geometry(TileMessage),
    Raster(RasterTileMessage),
    RasterDEM(RasterDEMTileMessage),
}

/// The worker type of a tile.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkerType {
    Geometry,
    Raster,
    RasterDEM,
}

/// The map side of one worker instance.
pub(crate) struct MapCodec {
    /// The keys of what the instance has of this side's: layers, their
    /// properties and expressions, and tile data.
    tables: shared::Tables,
    tiles: HashMap<u64, TileState>,
}

/// What a tile sent its worker that its replies refer to by key, kept until
/// no reply can: the layers of each `setLayers` and the data of each
/// `setData` a layout may still answer, oldest first. Nothing else here
/// need keep them (the render layers evaluate their layers again, say).
struct TileState {
    worker_type: WorkerType,
    layers: Vec<(u64, Vec<Arc<dyn LayerProperties>>)>,
    data: Vec<(u64, Option<TileData>)>,
}

/// Drops the entries a layout for `correlation_id` makes stale: all but the
/// last one sent at or before it (the one the worker had then), as no later
/// reply can answer them.
fn drop_stale<T>(entries: &mut Vec<(u64, T)>, correlation_id: u64) {
    if let Some(index) = entries.iter().rposition(|(sent, _)| *sent <= correlation_id) {
        entries.drain(..index);
    }
}

impl Default for MapCodec {
    fn default() -> Self {
        MapCodec {
            tables: shared::Tables::new()
                .origin::<SharedLayer>()
                .origin::<SharedLayerProperties>()
                .origin::<SharedExpression>()
                .origin::<mln_tile::TileBytes>()
                .origin::<mln_tile::TileFeatures>(),
            tiles: HashMap::default(),
        }
    }
}

impl MapCodec {
    pub(crate) fn new() -> Self {
        MapCodec::default()
    }

    /// A request: `action`, and then `message` if there is one. Fails if the
    /// message can't be encoded, which then defines nothing.
    fn request<M: Wire>(&mut self, action: Action, message: Option<&M>) -> Result<Vec<u8>, String> {
        let message = match message {
            Some(message) => self.tables.open(|| {
                let mut bytes = Vec::new();
                envelope::append(&mut bytes, message)?;
                Ok::<_, String>(bytes)
            })?,
            None => Vec::new(),
        };
        // After the message, which keeps alive what it defines.
        let forget = self.tables.forget_dead();
        let mut bytes = self.tables.open(|| {
            let mut bytes = Vec::new();
            envelope::append(&mut bytes, &Request { forget, action })?;
            Ok::<_, String>(bytes)
        })?;
        bytes.extend_from_slice(&message);
        Ok(bytes)
    }

    /// A request whose encoding doesn't fail: it holds nothing a
    /// `shared::Kind` could refuse to write.
    fn infallible_request<M: Wire>(&mut self, action: Action, message: Option<&M>) -> Vec<u8> {
        self.request(action, message).unwrap_or_else(|error| {
            Log::error(Event::General, format!("Can't encode a tile worker request: {error}"));
            Vec::new()
        })
    }

    fn bare_request(&mut self, action: Action) -> Vec<u8> {
        self.infallible_request::<()>(action, None)
    }

    /// The request that creates the worker of `tile`.
    pub(crate) fn create(&mut self, tile: u64, parameters: &WorkerParameters) -> Vec<u8> {
        self.add(tile, WorkerType::Geometry);
        self.bare_request(Action::Create { tile, parameters: parameters.clone() })
    }

    /// The request that creates the `RasterTileWorker` of `tile`.
    pub(crate) fn create_raster(&mut self, tile: u64) -> Vec<u8> {
        self.add(tile, WorkerType::Raster);
        self.bare_request(Action::CreateRaster { tile })
    }

    /// The request that creates the `RasterDEMTileWorker` of `tile`.
    pub(crate) fn create_raster_dem(&mut self, tile: u64) -> Vec<u8> {
        self.add(tile, WorkerType::RasterDEM);
        self.bare_request(Action::CreateRasterDEM { tile })
    }

    fn add(&mut self, tile: u64, worker_type: WorkerType) {
        self.tiles.insert(tile, TileState { worker_type, layers: Vec::new(), data: Vec::new() });
    }

    /// The request that drops the worker of `tile`.
    pub(crate) fn remove(&mut self, tile: u64) -> Vec<u8> {
        self.tiles.remove(&tile);
        self.bare_request(Action::Remove { tile })
    }

    /// The request that sends `message` to the worker of `tile`. Fails for
    /// what can't cross (tile data that no longer has its bytes, say) or an
    /// unknown tile.
    pub(crate) fn message(&mut self, tile: u64, message: &WorkerMessage) -> Result<Vec<u8>, String> {
        let state = self.tiles.get_mut(&tile).ok_or_else(|| format!("no tile {tile}"))?;
        let correlation_id = message.correlation_id();
        let kind = match message {
            WorkerMessage::SetData { data, .. } => {
                state.data.push((correlation_id, data.clone()));
                MessageKind::SetData
            }
            WorkerMessage::SetLayers { layers, .. } => {
                state.layers.push((correlation_id, layers.clone()));
                MessageKind::SetLayers
            }
            _ => MessageKind::Other,
        };
        let about = About { correlation_id, kind };
        let request = self.request(Action::Message { tile, about }, Some(message));
        // The worker never gets what failed.
        if let (Err(_), Some(state)) = (&request, self.tiles.get_mut(&tile)) {
            match about.kind {
                MessageKind::SetData => drop(state.data.pop()),
                MessageKind::SetLayers => drop(state.layers.pop()),
                MessageKind::Other => {}
            }
        }
        request
    }

    /// The request that sends `message` to the `RasterTileWorker` of `tile`.
    pub(crate) fn raster_message(&mut self, tile: u64, message: &RasterWorkerMessage) -> Vec<u8> {
        let &RasterWorkerMessage::Parse { correlation_id, .. } = message;
        self.simple_message(tile, correlation_id, message)
    }

    /// The request that sends `message` to the `RasterDEMTileWorker` of
    /// `tile`.
    pub(crate) fn raster_dem_message(&mut self, tile: u64, message: &RasterDEMWorkerMessage) -> Vec<u8> {
        let &RasterDEMWorkerMessage::Parse { correlation_id, .. } = message;
        self.simple_message(tile, correlation_id, message)
    }

    /// A message of a raster or raster-dem tile, which refers to nothing
    /// shared.
    fn simple_message<M: Wire>(&mut self, tile: u64, correlation_id: u64, message: &M) -> Vec<u8> {
        let about = About { correlation_id, kind: MessageKind::Other };
        self.infallible_request(Action::Message { tile, about }, Some(message))
    }

    /// Decodes a reply: the tile it is for and its message, or `None` for a
    /// reply to a tile that is gone (sent before the worker got the
    /// `Remove`). A message this side can't decode becomes an error for the
    /// tile. Fails only for bytes that aren't a reply.
    pub(crate) fn decode(&mut self, reply: Encoded) -> Result<Option<(u64, Reply)>, String> {
        let Encoded { bytes, payloads } = reply;
        payload::decode(payloads, || {
            let (head, message): (ReplyHead, &[u8]) = envelope::take(&bytes)?;
            let Some(worker_type) = self.tiles.get(&head.tile).map(|state| state.worker_type) else {
                return Ok(None);
            };
            let correlation_id = head.correlation_id;
            let reply = match worker_type {
                WorkerType::Geometry => Reply::Geometry(match self.decode_message(message) {
                    Ok(TileMessage::Layout { result, correlation_id, request, resolved }) => {
                        if let Some(state) = self.tiles.get_mut(&head.tile) {
                            drop_stale(&mut state.layers, correlation_id);
                            drop_stale(&mut state.data, correlation_id);
                        }
                        TileMessage::Layout { result, correlation_id, request, resolved }
                    }
                    Ok(message) => message,
                    Err(message) => error(message, correlation_id),
                }),
                WorkerType::Raster => Reply::Raster(self.decode_message(message).unwrap_or_else(|message| {
                    RasterTileMessage::Error { error: Error::native(message), correlation_id }
                })),
                WorkerType::RasterDEM => Reply::RasterDEM(self.decode_message(message).unwrap_or_else(|message| {
                    RasterDEMTileMessage::Error { error: Error::native(message), correlation_id }
                })),
            };
            Ok(Some((head.tile, reply)))
        })
    }

    fn decode_message<M: Wire>(&mut self, bytes: &[u8]) -> Result<M, String> {
        self.tables.open(|| envelope::decode(bytes))
    }
}

fn error(message: impl Into<String>, correlation_id: u64) -> TileMessage {
    TileMessage::Error { error: Error::native(message.into()), correlation_id }
}
