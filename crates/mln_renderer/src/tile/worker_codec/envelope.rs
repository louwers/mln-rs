// Port-only: what goes around the messages between the map side and a worker
// instance (mod.rs), and how both are encoded.
//
// A request is a `Request`, then the message it carries, if any; a reply is a
// `ReplyHead`, then the worker's message. Each is encoded with postcard, one
// after the other, so that the side that gets them reads the envelope even
// when the message can't be read: the instance tells the tile which message
// failed, and the map side still takes a layout's atlases, whose bins would
// otherwise stay reserved in the instance's atlas.

use mln_style_types::image::RectU16;
use mln_util::serialization::payload::{self, Payload};
use mln_util::serialization::shared;
use serde::{Deserialize, Serialize};

use crate::gfx::dynamic_texture::{DynamicTexturePtr, TextureHandle};
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
#[derive(Serialize, Deserialize)]
pub struct Request {
    /// The keys of the shared values that are gone on the map side
    /// (`shared::Tables::forget_dead`).
    pub forget: Vec<(String, Vec<u64>)>,
    /// The bins of the instance's atlas that the map side's layouts released.
    pub release: Vec<TextureRelease>,
    pub action: Action,
}

#[derive(Serialize, Deserialize)]
pub enum Action {
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct About {
    /// The message's correlation id, or 0 for the glyph and image replies,
    /// which have none (the tile reports an error without one without
    /// clearing its pending state).
    pub correlation_id: u64,
    pub kind: MessageKind,
}

/// The messages whose failure loses what the worker had (host.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageKind {
    SetData,
    SetLayers,
    /// Any other message, which changes neither the data nor the layers.
    Other,
}

/// Bins of a texture of the instance's atlas that the map side no longer
/// uses: one release per use of a bin. The texture is the instance's, by
/// key; `None` for one the instance no longer has.
#[derive(Serialize, Deserialize)]
pub struct TextureRelease {
    #[serde(serialize_with = "shared::serialize_option::<fields::SharedTexture, _>")]
    #[serde(deserialize_with = "known_texture")]
    pub texture: Option<DynamicTexturePtr>,
    pub handles: Vec<TextureHandle>,
}

fn known_texture<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<DynamicTexturePtr>, D::Error> {
    Ok(Option::<Known>::deserialize(deserializer)?.and_then(|known| known.0))
}

struct Known(Option<DynamicTexturePtr>);

impl<'de> Deserialize<'de> for Known {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        shared::deserialize_known::<fields::SharedTexture, D>(deserializer).map(Known)
    }
}

/// What a worker instance sends back before the message of `tile`'s worker
/// for its tile. `A` is a layout's atlases (borrowed when encoding).
#[derive(Serialize, Deserialize)]
pub struct ReplyHead<A> {
    pub tile: u64,
    /// Every texture of the instance's atlas, so that the map side has each
    /// before the message refers to it.
    #[serde(with = "textures")]
    pub textures: Vec<DynamicTexturePtr>,
    /// The pixels the instance's atlas queued since the last reply.
    pub uploads: Vec<TextureUpload>,
    /// The keys of the instance's textures that are gone.
    pub forget: Vec<(String, Vec<u64>)>,
    /// A layout's atlases, apart from the layout (`LayoutResult` crosses
    /// without them).
    pub atlases: Option<A>,
    /// The message's correlation id, for an error about it.
    pub correlation_id: u64,
}

/// Pixels the instance's atlas queued for a bin of a texture.
#[derive(Serialize, Deserialize)]
pub struct TextureUpload {
    #[serde(serialize_with = "shared::serialize::<fields::SharedTexture, _>")]
    #[serde(deserialize_with = "shared::deserialize::<fields::SharedTexture, _>")]
    pub texture: DynamicTexturePtr,
    pub bin: i32,
    pub rect: RectU16,
    #[serde(with = "payload::bytes")]
    pub pixels: Vec<u8>,
}

mod textures {
    use super::{Deserialize, DynamicTexturePtr, Serialize, fields, shared};

    #[derive(Serialize, Deserialize)]
    struct Texture(
        #[serde(serialize_with = "shared::serialize::<fields::SharedTexture, _>")]
        #[serde(deserialize_with = "shared::deserialize::<fields::SharedTexture, _>")]
        DynamicTexturePtr,
    );

    pub fn serialize<S: serde::Serializer>(textures: &[DynamicTexturePtr], serializer: S) -> Result<S::Ok, S::Error> {
        textures.iter().cloned().map(Texture).collect::<Vec<_>>().serialize(serializer)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<DynamicTexturePtr>, D::Error> {
        Ok(Vec::<Texture>::deserialize(deserializer)?.into_iter().map(|texture| texture.0).collect())
    }
}

/// Appends `value`, encoded.
pub fn append<T: Serialize + ?Sized>(bytes: &mut Vec<u8>, value: &T) -> Result<(), String> {
    *bytes = postcard::to_extend(value, std::mem::take(bytes)).map_err(|e| format!("can't encode: {e}"))?;
    Ok(())
}

/// Decodes a value at the start of `bytes`; returns it and the bytes after
/// it.
pub fn take<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<(T, &'a [u8]), String> {
    postcard::take_from_bytes(bytes).map_err(|e| format!("malformed: {e}"))
}

/// Decodes a value that fills `bytes`.
pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, String> {
    match take(bytes)? {
        (value, []) => Ok(value),
        (_, rest) => Err(format!("malformed: {} bytes after the end", rest.len())),
    }
}
