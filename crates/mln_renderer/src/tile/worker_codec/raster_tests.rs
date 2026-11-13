// The worker codec with raster and raster-dem tiles: a worker behind the
// codec makes the bucket a worker reached directly makes, and it crosses
// without loss, DEM border included.

use std::sync::Arc;

use mln_storage::Data;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::tileset::RasterEncoding;
use parking_lot::Mutex;
use serde::Serialize;

use super::envelope::{self, ReplyHead};
use super::map_side::{MapCodec, Reply};
use super::worker_side::{WorkerAction, WorkerCodec};
use super::*;
use crate::bucket::BucketBase;
use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::buckets::raster_bucket::RasterBucket;
use crate::tile::geometry_tile::LayoutAtlases;
use crate::tile::raster_dem_tile::{RasterDEMTileMessage, RasterDEMTileWorker, RasterDEMWorkerMessage};
use crate::tile::raster_tile::{RasterTileMessage, RasterTileWorker, RasterWorkerMessage};
use crate::tile::tile_worker_transport::TileEndpoint;

const TILE: u64 = 3;
const CORRELATION_ID: u64 = 5;

/// Where a test worker's messages go.
struct Sink<M>(Arc<Mutex<Vec<M>>>);

impl<M> Clone for Sink<M> {
    fn clone(&self) -> Self {
        Sink(self.0.clone())
    }
}

impl<M> Sink<M> {
    fn new() -> Self {
        Sink(Arc::new(Mutex::new(Vec::new())))
    }

    /// The one message the worker sent.
    fn take(&self) -> M {
        let mut messages = std::mem::take(&mut *self.0.lock());
        assert_eq!(messages.len(), 1);
        messages.remove(0)
    }
}

impl<M: Send> TileEndpoint<M> for Sink<M> {
    fn send(&self, message: M) {
        self.0.lock().push(message);
    }
}

fn fixture(path: &str) -> Data {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(path);
    Arc::new(std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

/// A PNG of opaque pixels that differ from their neighbours, so that a DEM
/// made from it has no two equal edges.
fn png(width: u32, height: u32) -> Data {
    let mut image = PremultipliedImage::new(Size::new(width, height));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        *pixel = [(i % 251) as u8, (i / 7 % 253) as u8, (i * 3 % 255) as u8, 255];
    }
    Arc::new(mln_image::encode_png(&image))
}

/// A raster worker's message for its tile when the tile reaches it directly.
fn raster_direct(data: Option<Data>) -> RasterTileMessage {
    let sink = Sink::new();
    RasterTileWorker::new(Box::new(sink.clone()))
        .handle(RasterWorkerMessage::Parse { data, correlation_id: CORRELATION_ID });
    sink.take()
}

/// A raster worker's message for its tile through the codec: the request
/// encoded on the map side and decoded on the worker's, and the reply back.
fn raster_through_codec(data: Option<Data>) -> RasterTileMessage {
    let mut map = MapCodec::new();
    let mut worker_codec = WorkerCodec::new();
    let WorkerAction::CreateRaster { tile: TILE } = worker_codec.decode(&map.create_raster(TILE)).unwrap() else {
        panic!("not a raster worker");
    };
    let sink = Sink::new();
    let mut worker = RasterTileWorker::new(Box::new(sink.clone()));
    let request = map.raster_message(TILE, &RasterWorkerMessage::Parse { data, correlation_id: CORRELATION_ID });
    let WorkerAction::Message { tile: TILE, message, .. } = worker_codec.decode(&request).unwrap() else {
        panic!("not a message");
    };
    worker.handle(worker_codec.decode_message(&message).unwrap());
    let reply = worker_codec.encode_raster(TILE, &sink.take());
    let Some((TILE, Reply::Raster(message))) = map.decode(reply).unwrap() else {
        panic!("not a raster tile message");
    };
    message
}

fn raster_dem_direct(data: Option<Data>, encoding: RasterEncoding) -> RasterDEMTileMessage {
    let sink = Sink::new();
    RasterDEMTileWorker::new(Box::new(sink.clone())).handle(RasterDEMWorkerMessage::Parse {
        data,
        correlation_id: CORRELATION_ID,
        encoding,
    });
    sink.take()
}

fn raster_dem_through_codec(data: Option<Data>, encoding: RasterEncoding) -> RasterDEMTileMessage {
    let mut map = MapCodec::new();
    let mut worker_codec = WorkerCodec::new();
    let WorkerAction::CreateRasterDEM { tile: TILE } = worker_codec.decode(&map.create_raster_dem(TILE)).unwrap()
    else {
        panic!("not a raster-dem worker");
    };
    let sink = Sink::new();
    let mut worker = RasterDEMTileWorker::new(Box::new(sink.clone()));
    let request =
        map.raster_dem_message(TILE, &RasterDEMWorkerMessage::Parse { data, correlation_id: CORRELATION_ID, encoding });
    let WorkerAction::Message { tile: TILE, message, .. } = worker_codec.decode(&request).unwrap() else {
        panic!("not a message");
    };
    worker.handle(worker_codec.decode_message(&message).unwrap());
    let reply = worker_codec.encode_raster_dem(TILE, &sink.take());
    let Some((TILE, Reply::RasterDEM(message))) = map.decode(reply).unwrap() else {
        panic!("not a raster-dem tile message");
    };
    message
}

fn parsed_image(message: RasterTileMessage) -> Option<PremultipliedImage> {
    match message {
        RasterTileMessage::Parsed { bucket, correlation_id: CORRELATION_ID } => {
            bucket.map(|bucket| (*bucket.image.clone().expect("an image")).clone())
        }
        RasterTileMessage::Parsed { correlation_id, .. } => panic!("correlation id {correlation_id}"),
        RasterTileMessage::Error { error, .. } => panic!("{error}"),
    }
}

/// A raster tile's image, its absence and its decoding error cross.
#[test]
fn raster_tiles_cross() {
    for data in [fixture("test/fixtures/annotations/emerald.png"), png(16, 16)] {
        let direct = parsed_image(raster_direct(Some(data.clone()))).unwrap();
        let through = parsed_image(raster_through_codec(Some(data))).unwrap();
        assert!(direct.valid());
        assert_eq!(through.size, direct.size);
        assert_eq!(through.data, direct.data);
    }

    assert!(parsed_image(raster_through_codec(None)).is_none());

    let garbage = Some(Arc::new(b"not an image".to_vec()));
    let (RasterTileMessage::Error { error: direct, .. }, RasterTileMessage::Error { error: through, correlation_id }) =
        (raster_direct(garbage.clone()), raster_through_codec(garbage))
    else {
        panic!("not errors");
    };
    assert_eq!(correlation_id, CORRELATION_ID);
    assert_eq!(through.to_string(), direct.to_string());
}

fn parsed_dem(message: RasterDEMTileMessage) -> Option<Arc<HillshadeBucket>> {
    match message {
        RasterDEMTileMessage::Parsed { bucket, correlation_id: CORRELATION_ID } => bucket,
        RasterDEMTileMessage::Parsed { correlation_id, .. } => panic!("correlation id {correlation_id}"),
        RasterDEMTileMessage::Error { error, .. } => panic!("{error}"),
    }
}

/// A raster-dem tile's DEM crosses with its border, so that it backfills
/// from its neighbours, and gives them its border, as a DEM made directly.
#[test]
fn raster_dem_tiles_cross() {
    let custom = RasterEncoding::Custom { red_factor: 0.0, green_factor: 25.6, blue_factor: 0.1, base_shift: 3446.4 };
    for encoding in [RasterEncoding::Mapbox, RasterEncoding::Terrarium, custom] {
        let direct = parsed_dem(raster_dem_direct(Some(png(16, 16)), encoding)).unwrap();
        let through = parsed_dem(raster_dem_through_codec(Some(png(16, 16)), encoding)).unwrap();
        {
            let (direct, through) = (direct.get_dem_data(), through.get_dem_data());
            assert_eq!((through.dim, through.stride, through.encoding), (16, 18, encoding));
            assert_eq!((through.dim, through.stride, through.encoding), (direct.dim, direct.stride, direct.encoding));
            assert_eq!(through.get_image().size, direct.get_image().size);
            assert_eq!(through.get_image().data, direct.get_image().data);
        }
        let encoded = |bucket: &HillshadeBucket| postcard::to_allocvec(bucket).unwrap();
        assert_eq!(encoded(&through), encoded(&direct));

        // A neighbour on the left, made directly and through the codec, each
        // backfilled from the other.
        let other = |pixels: &[u8]| -> Data {
            let mut image = PremultipliedImage::new(Size::new(16, 16));
            for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                *pixel = [pixels[0], (i % 256) as u8, pixels[1], 255];
            }
            Arc::new(mln_image::encode_png(&image))
        };
        let left_direct = parsed_dem(raster_dem_direct(Some(other(&[9, 200])), encoding)).unwrap();
        let left_through = parsed_dem(raster_dem_through_codec(Some(other(&[9, 200])), encoding)).unwrap();
        direct.get_dem_data_mut().backfill_border(&left_direct.get_dem_data(), -1, 0);
        through.get_dem_data_mut().backfill_border(&left_through.get_dem_data(), -1, 0);
        left_direct.get_dem_data_mut().backfill_border(&direct.get_dem_data(), 1, 0);
        left_through.get_dem_data_mut().backfill_border(&through.get_dem_data(), 1, 0);
        assert_eq!(through.get_dem_data().get_image().data, direct.get_dem_data().get_image().data);
        assert_eq!(left_through.get_dem_data().get_image().data, left_direct.get_dem_data().get_image().data);
        assert_eq!(through.get_dem_data().get(-1, 3), left_through.get_dem_data().get(15, 3));
    }

    assert!(parsed_dem(raster_dem_through_codec(None, RasterEncoding::Mapbox)).is_none());

    // The DEMData constructor's error, for an image that isn't square.
    let (RasterDEMTileMessage::Error { error: direct, .. }, RasterDEMTileMessage::Error { error: through, .. }) = (
        raster_dem_direct(Some(png(16, 8)), RasterEncoding::Mapbox),
        raster_dem_through_codec(Some(png(16, 8)), RasterEncoding::Mapbox),
    ) else {
        panic!("not errors");
    };
    assert_eq!(through.to_string(), "raster-dem tiles must be square.");
    assert_eq!(through.to_string(), direct.to_string());
}

/// A reply of `TILE` with `message`, as a worker instance encodes it.
fn reply<M: Serialize>(message: &M) -> Encoded {
    let head = ReplyHead::<&LayoutAtlases> {
        tile: TILE,
        textures: Vec::new(),
        uploads: Vec::new(),
        forget: Vec::new(),
        atlases: None,
        correlation_id: CORRELATION_ID,
    };
    let (bytes, payloads) = mln_util::serialization::payload::encode(|| {
        let mut bytes = Vec::new();
        envelope::append(&mut bytes, &head).unwrap();
        envelope::append(&mut bytes, message).unwrap();
        bytes
    });
    Encoded { bytes, payloads }
}

/// What a `RasterDEMTileMessage::Parsed` with a `HillshadeBucket` crosses
/// as, with a `DEMData` of any members.
#[derive(Serialize)]
enum DEMParsed {
    Parsed { bucket: Option<(BucketBase, (i32, i32, RasterEncoding, PremultipliedImage))>, correlation_id: u64 },
}

/// A reply of another worker type than the tile's becomes an error for the
/// tile; one for a tile that is gone is dropped.
#[test]
fn malformed_raster_replies() {
    let mut map = MapCodec::new();
    map.create_raster_dem(TILE);
    let image = PremultipliedImage::new(Size::new(2, 2));
    let raster =
        RasterTileMessage::Parsed { bucket: Some(Arc::new(RasterBucket::new(image))), correlation_id: CORRELATION_ID };
    let Some((TILE, Reply::RasterDEM(RasterDEMTileMessage::Error { .. }))) = map.decode(reply(&raster)).unwrap() else {
        panic!("not an error for the tile");
    };
    assert!(map.decode(Encoded::from(vec![0xff; 4])).is_err());

    // A tile that is gone.
    map.remove(TILE);
    assert!(map.decode(reply(&raster)).unwrap().is_none());
}

/// A bucket the map side can't read becomes an error for its tile: sizes
/// whose byte counts overflow 32 bits, or a DEM `new` can't make, are
/// malformed buckets, not images without their pixels.
#[test]
fn malformed_raster_buckets() {
    let mut map = MapCodec::new();
    map.create_raster(TILE);
    let image = PremultipliedImage::from_vec(Size::new(0x4000_0000, 1), Vec::new());
    let parsed =
        RasterTileMessage::Parsed { bucket: Some(Arc::new(RasterBucket::new(image))), correlation_id: CORRELATION_ID };
    let Some((TILE, Reply::Raster(RasterTileMessage::Error { .. }))) = map.decode(reply(&parsed)).unwrap() else {
        panic!("not an error for the tile");
    };

    let mut map = MapCodec::new();
    map.create_raster_dem(TILE);
    let image = |size, bytes| PremultipliedImage::from_vec(Size::new(size, size), vec![0; bytes]);
    let dems = [
        (4, 5, image(6, 6 * 6 * 4)),
        (-1, 1, image(1, 4)),
        (i32::MAX, 1, image(1, 4)),
        (32766, 32768, image(32768, 0)),
    ];
    for (dim, stride, image) in dems {
        let malformed = DEMParsed::Parsed {
            bucket: Some((BucketBase::default(), (dim, stride, RasterEncoding::Mapbox, image))),
            correlation_id: CORRELATION_ID,
        };
        let Some((TILE, Reply::RasterDEM(RasterDEMTileMessage::Error { correlation_id: CORRELATION_ID, .. }))) =
            map.decode(reply(&malformed)).unwrap()
        else {
            panic!("dim {dim}, stride {stride}: not an error for the tile");
        };
    }
}

/// A decoded image travels beside the reply's bytes, as a payload that the
/// map side takes as it is; small arrays stay in the bytes.
#[test]
fn large_arrays_travel_as_payloads() {
    let parsed = |pixels: Vec<u8>, width| RasterTileMessage::Parsed {
        bucket: Some(Arc::new(RasterBucket::new(PremultipliedImage::from_vec(Size::new(width, width), pixels)))),
        correlation_id: CORRELATION_ID,
    };
    let decoded = |map: &mut MapCodec, encoded| match map.decode(encoded) {
        Ok(Some((TILE, Reply::Raster(message)))) => parsed_image(message).unwrap().data,
        _ => panic!("not a raster tile message"),
    };
    let mut map = MapCodec::new();
    map.create_raster(TILE);
    let pixels = vec![7; 64 * 64 * 4];
    let encoded = WorkerCodec::new().encode_raster(TILE, &parsed(pixels.clone(), 64));
    assert_eq!(encoded.payloads, [pixels.clone().into()]);
    assert!(encoded.bytes.len() < 64);
    assert_eq!(decoded(&mut map, encoded.clone()), pixels);
    // Without its payload, the image doesn't decode.
    let Ok(Some((TILE, Reply::Raster(RasterTileMessage::Error { .. })))) = map.decode(encoded.bytes.into()) else {
        panic!("not an error for the tile");
    };

    let encoded = WorkerCodec::new().encode_raster(TILE, &parsed(vec![1; 4], 1));
    assert_eq!(encoded.payloads.as_slice(), []);
    assert_eq!(decoded(&mut map, encoded), vec![1; 4]);
}
