// Port-only: a worker instance's side of the worker codec (mod.rs): decodes
// what the map side sends the instance's workers and encodes what they send
// back.

use std::sync::Arc;

use mln_style::serialization::{SharedExpression, SharedLayer, SharedLayerProperties};
use mln_style_types::image::SharedImage;
use mln_util::error::Error;
use mln_util::serialization::{payload, shared};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::envelope::{self, About, Action, Encoded, ReplyHead, Request, TextureUpload};
use super::fields::{SharedTexture, SharedTileDataKind};
use crate::gfx::dynamic_texture_atlas::{DynamicTextureAtlas, DynamicTextureAtlasPtr};
use crate::tile::geometry_tile::{LayoutAtlases, LayoutResult};
use crate::tile::geometry_tile_worker::TileMessage;
use crate::tile::raster_dem_tile::RasterDEMTileMessage;
use crate::tile::raster_tile::RasterTileMessage;
use crate::tile::tile_worker_transport::WorkerParameters;

/// What a worker instance gets from the map side.
pub enum WorkerAction {
    /// Create the worker of `tile`; its parameters have the instance's atlas.
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
    /// Nothing to do (the request only forgot values or released bins).
    None,
}

/// A worker instance's side.
pub struct WorkerCodec {
    /// The values the map side defined, by key, and the keys of the textures
    /// of the instance's atlas.
    tables: shared::Tables,
    /// The texture atlas of the instance's workers. The map side keeps a
    /// copy of each of its textures (map_side.rs).
    atlas: DynamicTextureAtlasPtr,
}

impl Default for WorkerCodec {
    fn default() -> Self {
        WorkerCodec::with_atlas(Arc::new(DynamicTextureAtlas::new()))
    }
}

impl WorkerCodec {
    pub fn new() -> Self {
        WorkerCodec::default()
    }

    /// A codec for an instance whose workers use `atlas`.
    pub(super) fn with_atlas(atlas: DynamicTextureAtlasPtr) -> Self {
        let tables = shared::Tables::new()
            .copies::<SharedLayer>()
            .copies::<SharedLayerProperties>()
            .copies::<SharedExpression>()
            .copies::<SharedImage>()
            .copies::<SharedTileDataKind>()
            .origin::<SharedTexture>();
        WorkerCodec { tables, atlas }
    }

    /// The texture atlas the instance's workers upload their glyphs and
    /// images to.
    #[cfg(test)]
    pub(super) fn atlas(&self) -> &DynamicTextureAtlasPtr {
        &self.atlas
    }

    /// Decodes a request, and does what it forgets and releases. Fails only
    /// for bytes that aren't a request.
    pub fn decode(&mut self, bytes: &[u8]) -> Result<WorkerAction, String> {
        let (request, message): (Request, &[u8]) = self.tables.open(|| envelope::take(bytes))?;
        self.tables.forget(&request.forget);
        if !request.release.is_empty() {
            let textures = self.atlas.textures();
            for release in request.release {
                // A texture the atlas no longer has has no bins to free.
                if let Some(texture) = release.texture
                    && textures.iter().any(|known| Arc::ptr_eq(known, &texture))
                {
                    self.atlas.remove_textures(&release.handles, &Some(texture));
                }
            }
            // The atlas frees only the textures nothing else holds.
            drop(textures);
            self.atlas.remove_unused_dynamic_textures();
        }
        Ok(match request.action {
            Action::Create { tile, mut parameters } => {
                parameters.dynamic_texture_atlas = Some(self.atlas.clone());
                WorkerAction::Create { tile, parameters }
            }
            Action::CreateRaster { tile } => WorkerAction::CreateRaster { tile },
            Action::CreateRasterDEM { tile } => WorkerAction::CreateRasterDEM { tile },
            Action::Message { tile, about } => WorkerAction::Message { tile, about, message: message.to_vec() },
            Action::Remove { tile } => WorkerAction::Remove { tile },
            Action::None => WorkerAction::None,
        })
    }

    /// Decodes the message of a `WorkerAction::Message` as an `M`.
    pub fn decode_message<M: DeserializeOwned>(&mut self, message: &[u8]) -> Result<M, String> {
        self.tables.open(|| envelope::decode(message))
    }

    /// Encodes what the worker of `tile` sends its tile, its arrays as
    /// payloads. What can't cross becomes an error for the tile.
    pub fn encode(&mut self, tile: u64, message: TileMessage) -> Encoded {
        let (correlation_id, atlases) = match &message {
            TileMessage::Layout { result, correlation_id } => (*correlation_id, Some(result.atlases.clone())),
            TileMessage::Error { correlation_id, .. } => (*correlation_id, None),
            TileMessage::GetGlyphs(_) | TileMessage::GetImages(_) => (0, None),
        };
        let encoded = self.encode_reply(tile, correlation_id, atlases.as_deref(), &message, |error| {
            TileMessage::Error { error: Error::native(error), correlation_id }
        });
        drop(atlases);
        if let TileMessage::Layout { result, .. } = message {
            // The map side's copy of the layout's atlases releases the bins.
            keep_bins(result);
        }
        encoded
    }

    /// Encodes what the `RasterTileWorker` of `tile` sends its tile.
    pub fn encode_raster(&mut self, tile: u64, message: RasterTileMessage) -> Encoded {
        let (RasterTileMessage::Parsed { correlation_id, .. } | RasterTileMessage::Error { correlation_id, .. }) =
            message;
        self.encode_reply(tile, correlation_id, None, &message, |error| RasterTileMessage::Error {
            error: Error::native(error),
            correlation_id,
        })
    }

    /// Encodes what the `RasterDEMTileWorker` of `tile` sends its tile.
    pub fn encode_raster_dem(&mut self, tile: u64, message: RasterDEMTileMessage) -> Encoded {
        let (RasterDEMTileMessage::Parsed { correlation_id, .. } | RasterDEMTileMessage::Error { correlation_id, .. }) =
            message;
        self.encode_reply(tile, correlation_id, None, &message, |error| RasterDEMTileMessage::Error {
            error: Error::native(error),
            correlation_id,
        })
    }

    /// A reply: its head, with the instance's textures and the pixels queued
    /// for them, and then `message`, or the `error` of what failed to
    /// encode (the head keeps a layout's atlases, which the map side then
    /// releases).
    fn encode_reply<M: Serialize>(
        &mut self,
        tile: u64,
        correlation_id: u64,
        atlases: Option<&LayoutAtlases>,
        message: &M,
        error: impl FnOnce(String) -> M,
    ) -> Encoded {
        let textures = self.atlas.textures();
        let mut uploads = Vec::new();
        for texture in &textures {
            for (bin, rect, pixels) in texture.take_uploads() {
                uploads.push(TextureUpload { texture: texture.clone(), bin, rect, pixels });
            }
        }
        let forget = self.tables.forget_dead();
        let head = ReplyHead { tile, textures, uploads, forget, atlases, correlation_id };
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

/// Drops a layout without releasing its atlases' bins.
fn keep_bins(result: LayoutResult) {
    let LayoutResult { atlases, .. } = result;
    if let Ok(mut atlases) = Arc::try_unwrap(atlases) {
        atlases.dynamic_texture_atlas = None;
    }
}
