//! Maps over terrain whose DEM tiles are missing, or are let go of once they
//! load, which end their still images and go idle all the same.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use mln_engine::map::{MapEvent, MapMode};
use mln_engine::storage::Response;
use mln_runtime::{DemandMode, FrameDisposition, MapOptions, RuntimeEventKind};
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::support::eventually;
use crate::support::still::{StillMap, StillOptions, not_found, pixel, serve};

/// Seen from zoom 1, one terrain tile covers the world, and reads the one
/// DEM tile, at zoom 0.
const TERRAIN_STYLE: &str = r#"{
    "version": 8,
    "zoom": 1,
    "terrain": {"source": "dem"},
    "sources": {"dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "maxzoom": 0}},
    "layers": [{"id": "background", "type": "background", "paint": {"background-color": "red"}}]
}"#;

/// A still image over a terrain whose source has no DEM tile ends, in one
/// frame, with the camera on flat ground.
#[test]
fn a_still_image_over_terrain_without_its_dem_tile_ends() {
    let map = StillMap::new(StillOptions { size: Size::new(64, 64), ..StillOptions::default() });
    serve(&map.runtime, |_| not_found());
    map.load_json(TERRAIN_STYLE);
    let image = map.render();

    let mut results = Vec::new();
    assert!(eventually(|| {
        results.extend(map.session.drain_frame_results().expect("the frame results"));
        results.iter().any(|result| result.token.is_none())
    }));
    let still = results.iter().find(|result| result.token.is_none()).expect("the still's frame");
    assert_eq!(still.disposition, FrameDisposition::Rendered);
    assert_eq!(still.frame_generation, 1, "the still draws one frame");
    assert_eq!(map.map.snapshot().expect("a snapshot").view.camera.center_altitude, 0.0);
    assert_eq!(pixel(&image, 32, 32), [255, 0, 0, 255]);
}

/// Seen from zoom 10.5 at a pitch of 60°, ground that is flat but for the
/// DEM tiles far from the camera, which stand high ([`serve_far_dem`]).
const FAR_DEM_STYLE: &str = r#"{
    "version": 8,
    "center": [10, 45],
    "zoom": 10.5,
    "pitch": 60,
    "terrain": {"source": "dem", "exaggeration": 10},
    "sources": {"dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "encoding": "terrarium", "maxzoom": 14}},
    "layers": [{"id": "background", "type": "background", "paint": {"background-color": "red"}}]
}"#;

/// The requests for tiles a provider answered: for all of them, and for the
/// far ones of [`serve_far_dem`].
#[derive(Clone, Default)]
struct DemRequests {
    all: Arc<AtomicUsize>,
    far: Arc<AtomicUsize>,
}

impl DemRequests {
    fn all(&self) -> usize {
        self.all.load(Ordering::SeqCst)
    }

    fn far(&self) -> usize {
        self.far.load(Ordering::SeqCst)
    }
}

/// Serves the DEM of [`FAR_DEM_STYLE`], and counts the requests for its
/// tiles. Its tiles of zoom 6 or more whose south edge lies north of the
/// center's row of terrain tiles stand high; terrain tiles read only them
/// there. Once one loads, its height range culls the terrain tiles that read
/// it, so that the terrain no longer reads it.
fn serve_far_dem(map: &StillMap) -> DemRequests {
    // DEM tiles of 16 pixels a side, each `height` m in Terrarium.
    let terrarium = |height: u16| {
        let mut dem = PremultipliedImage::new(Size::new(16, 16));
        let [hi, lo] = (height + 32768).to_be_bytes();
        for pixel in dem.data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[hi, lo, 0, 255]);
        }
        Arc::new(mln_engine::image::encode_png(&dem))
    };
    let (low, high) = (terrarium(0), terrarium(20_000));
    let center_y = mln_engine::geo::projection::Projection::project(
        &mln_style::geo::LatLng::new(45.0, 10.0).unwrap(),
        1.0 / mln_style::constants::TILE_SIZE_D,
    )
    .y;
    let requests = DemRequests::default();
    let counts = requests.clone();
    serve(&map.runtime, move |resource| {
        let tile = resource.tile_data.as_ref().expect("a tile");
        let south = f64::from(tile.y + 1) / f64::from(1u32 << tile.z);
        let far = tile.z > 5 && south < center_y - 0.0003;
        counts.all.fetch_add(1, Ordering::SeqCst);
        if far {
            counts.far.fetch_add(1, Ordering::SeqCst);
        }
        Response { data: Some(Arc::clone(if far { &high } else { &low })), ..Response::default() }
    });
    requests
}

/// Whether the map reported, after its event `since`, that it went idle.
fn idle_since(map: &StillMap, since: usize) -> bool {
    map.events.events()[since..].iter().any(|event| matches!(event.kind, RuntimeEventKind::Map(MapEvent::Idle)))
}

/// A continuous map over far DEM tiles that the terrain lets go of once
/// they load ([`serve_far_dem`]), without the tile cache to keep them, goes
/// idle and stays so: the terrain remembers their height ranges, so the
/// cover doesn't read them again, and no frame follows.
#[test]
fn a_continuous_map_over_far_dem_tiles_that_flip_settles() {
    let map = StillMap::new(StillOptions {
        size: Size::new(1024, 1024),
        mode: MapMode::Continuous,
        map: MapOptions { tile_cache_enabled: false, ..StillOptions::default().map },
        ..StillOptions::default()
    });
    let requests = serve_far_dem(&map);
    map.load_json(FAR_DEM_STYLE);

    // Draws each frame that the map asks for, as a display does, until it
    // goes idle. A software rasterizer, as on CI's Windows runners, takes
    // about 30 s to draw the frames before then at this size.
    let deadline = Instant::now() + Duration::from_secs(120);
    while !idle_since(&map, 0) {
        assert!(Instant::now() < deadline, "the map never went idle");
        if map.frame_with(DemandMode::IfChanged).disposition != FrameDisposition::Rendered {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert!(requests.far() > 0, "the far DEM tiles load");

    // A while later, the map has drawn nothing more, nor loaded a tile.
    let idle = map.events.events().len();
    let all = requests.all();
    let quiet = Instant::now() + Duration::from_millis(300);
    while Instant::now() < quiet {
        let result = map.frame_with(DemandMode::IfChanged);
        assert_ne!(result.disposition, FrameDisposition::Rendered, "frames follow the idle map's");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!idle_since(&map, idle), "the map went idle again");
    assert_eq!(all, requests.all(), "DEM tiles load after the map went idle");
}
