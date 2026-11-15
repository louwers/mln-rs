// Port-only: what goes around the messages between the map side and a worker
// instance (mod.rs), and how both are encoded.
//
// A request is a `Request`, then the message it carries, if any; a reply is a
// `ReplyHead`, then the worker's message. Each is encoded with mln_wire, one
// after the other, so that the side that gets them reads the envelope even
// when the message can't be read: the instance tells the tile which message
// failed, and the map side still takes a layout's atlases, whose bins would
// otherwise stay reserved in the instance's atlas.

use std::borrow::Borrow;

use mln_style_types::image::RectU16;
use mln_util::serialization::payload::{self, Payload};
use mln_util::serialization::shared;
use mln_wire::{Reader, Wire, Writer};

use crate::gfx::dynamic_texture::{DynamicTextureBinId, DynamicTexturePtr, TextureHandle};
use crate::tile::geometry_tile::LayoutAtlases;
use crate::tile::tile_worker_transport::WorkerParameters;
use crate::tile::worker_codec::fields;

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
    /// The bins of the instance's atlas that the map side's layouts released.
    pub(crate) release: Vec<TextureRelease>,
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
    /// Nothing but what the request forgets and releases.
    None,
}

/// What a message is, for an error about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Wire)]
pub(crate) struct About {
    /// The message's correlation id, or 0 for the glyph and image replies,
    /// which have none (the tile reports an error without one without
    /// clearing its pending state).
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

/// Bins of a texture of the instance's atlas that the map side no longer
/// uses: one release per use of a bin. The texture is the instance's, by
/// key; `None` for one the instance no longer has.
#[derive(Wire)]
pub(crate) struct TextureRelease {
    #[wire(with = "known_texture")]
    pub(crate) texture: Option<DynamicTexturePtr>,
    pub(crate) handles: Vec<TextureHandle>,
}

/// An optional texture by key, which the instance may no longer have.
mod known_texture {
    use super::{DynamicTexturePtr, Reader, Writer, fields, shared};

    #[expect(clippy::ref_option, reason = "`#[wire(with)]` passes the field by reference")]
    pub(super) fn write(texture: &Option<DynamicTexturePtr>, w: &mut Writer) {
        shared::write_option::<fields::SharedTexture>(texture, w);
    }

    #[expect(clippy::option_option, reason = "a read that can fail, of an `Option`")]
    pub(super) fn read(r: &mut Reader<'_>) -> Option<Option<DynamicTexturePtr>> {
        match r.byte()? {
            0 => Some(None),
            1 => shared::read_known::<fields::SharedTexture>(r),
            _ => r.fail("invalid option"),
        }
    }
}

/// What a worker instance sends back before the message of `tile`'s worker
/// for its tile. `A` is a layout's atlases (borrowed when encoding).
pub(crate) struct ReplyHead<A> {
    pub(crate) tile: u64,
    /// Every texture of the instance's atlas, so that the map side has each
    /// before the message refers to it.
    pub(crate) textures: Vec<DynamicTexturePtr>,
    /// The pixels the instance's atlas queued since the last reply.
    pub(crate) uploads: Vec<TextureUpload>,
    /// The keys of the instance's textures that are gone.
    pub(crate) forget: Vec<(String, Vec<u64>)>,
    /// A layout's atlases, apart from the layout (`LayoutResult` crosses
    /// without them).
    pub(crate) atlases: Option<A>,
    /// The message's correlation id, for an error about it.
    pub(crate) correlation_id: u64,
}

impl<A: Borrow<LayoutAtlases>> ReplyHead<A> {
    pub(crate) fn write(&self, w: &mut Writer) {
        self.tile.write(w);
        textures::write(&self.textures, w);
        self.uploads.write(w);
        self.forget.write(w);
        self.atlases.as_ref().map(Borrow::borrow).write_ref(w);
        self.correlation_id.write(w);
    }
}

impl ReplyHead<LayoutAtlases> {
    pub(crate) fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(ReplyHead {
            tile: Wire::read(r)?,
            textures: textures::read(r)?,
            uploads: Wire::read(r)?,
            forget: Wire::read(r)?,
            atlases: Wire::read(r)?,
            correlation_id: Wire::read(r)?,
        })
    }
}

/// Writes an `Option` of a borrowed value as `Option<T>` reads.
trait WriteRef {
    fn write_ref(self, w: &mut Writer);
}

impl<T: Wire> WriteRef for Option<&T> {
    fn write_ref(self, w: &mut Writer) {
        match self {
            None => w.push(0),
            Some(value) => {
                w.push(1);
                value.write(w);
            }
        }
    }
}

/// Pixels the instance's atlas queued for a bin of a texture.
#[derive(Wire)]
pub(crate) struct TextureUpload {
    #[wire(with = "fields::shared_texture")]
    pub(crate) texture: DynamicTexturePtr,
    pub(crate) bin: DynamicTextureBinId,
    pub(crate) rect: RectU16,
    #[wire(with = "payload::bytes")]
    pub(crate) pixels: Vec<u8>,
}

mod textures {
    use super::{DynamicTexturePtr, Reader, Writer, fields, shared};

    pub(super) fn write(textures: &[DynamicTexturePtr], w: &mut Writer) {
        mln_wire::Wire::write(&textures.len(), w);
        for texture in textures {
            shared::write::<fields::SharedTexture>(texture, w);
        }
    }

    pub(super) fn read(r: &mut Reader<'_>) -> Option<Vec<DynamicTexturePtr>> {
        let len = r.length()?;
        let mut textures = Vec::with_capacity(len.min(r.rest().len()));
        for _ in 0..len {
            textures.push(shared::read::<fields::SharedTexture>(r)?);
        }
        Some(textures)
    }
}

/// Appends `value`, encoded.
pub(crate) fn append<T: Wire>(bytes: &mut Vec<u8>, value: &T) -> Result<(), String> {
    append_with(bytes, |w| value.write(w))
}

/// Appends what `write` writes.
pub(crate) fn append_with(bytes: &mut Vec<u8>, write: impl FnOnce(&mut Writer)) -> Result<(), String> {
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
pub(crate) fn take_with<T>(
    bytes: &[u8],
    read: impl FnOnce(&mut Reader<'_>) -> Option<T>,
) -> Result<(T, &[u8]), String> {
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
