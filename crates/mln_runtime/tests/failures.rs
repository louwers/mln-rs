// Frames that fail on the GPU, through a session's own WebGPU device: a
// layer that records a draw that WebGPU rejects fails the frame's
// submit, as a device out of memory would fail it. A still image ends with
// the error, and a continuous map reports it once, shows what drew, and
// draws again later.

#![cfg(all(feature = "webgpu", not(target_family = "wasm")))]

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mln_engine::RenderMode;
use mln_engine::draw::LendAt;
use mln_engine::map::{MapEvent, MapMode};
use mln_runtime::{
    AttachOptions, BackendType, DemandMode, Device, DeviceOptions, ErrorKind, FrameDemand, FrameDisposition,
    FrameResult, LogicalExtent, Map, MapOptions, RenderSession, RenderTarget, Runtime, RuntimeEventKind,
};
use mln_style::size::Size;

use crate::support::lends::{self, LendingHost, runtime};
use crate::support::{EventLog, eventually, result_of, wait};

const SIZE: Size = Size { width: 64, height: 64 };

const RED: [u8; 4] = [255, 0, 0, 255];

const STYLE: &str = r##"{"version":8,"transition":{"duration":0,"delay":0},"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"#ff0000"}}]}"##;

/// A layer that, while `broken`, records a draw without a pipeline in the
/// frame's pass, which WebGPU rejects when the frame is submitted.
struct Breaking {
    broken: Arc<AtomicBool>,
}

impl LendingHost for Breaking {
    fn lend(&self, at: LendAt, raw: &mut dyn Any) {
        if at == LendAt::Pass
            && self.broken.load(Ordering::SeqCst)
            && let Some(lent) = raw.downcast_mut::<mln_webgpu::CustomLayerPass>()
        {
            lent.pass.draw(0..3, 0..1);
        }
    }
}

/// A map of `mode` with a red background and a [`Breaking`] layer, and an
/// offscreen session of `SIZE` on a WebGPU device; `None` where the
/// machine has no WebGPU device.
fn broken_map(runtime: &Runtime, mode: MapMode) -> Option<(Map, RenderSession, Arc<AtomicBool>)> {
    let map = red_map(runtime, mode, SIZE);
    let broken = Arc::new(AtomicBool::new(false));
    wait(lends::add_layer(&map, "breaking", Breaking { broken: Arc::clone(&broken) }));
    let session = offscreen_session(&map, SIZE)?;
    Some((map, session, broken))
}

/// A map of `mode` and `size` with a red background.
fn red_map(runtime: &Runtime, mode: MapMode, size: Size) -> Map {
    let options = MapOptions {
        map: mln_engine::map::MapOptions { size, map_mode: mode, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    result_of(created).unwrap();
    wait(map.set_style_json(STYLE));
    map
}

/// An offscreen session of `map`, of `size`, on a WebGPU device; `None`
/// where the machine has no WebGPU device.
fn offscreen_session(map: &Map, size: Size) -> Option<RenderSession> {
    let device = match Device::with_backend(BackendType::WebGPU, DeviceOptions::default()) {
        Ok(device) => device,
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no WebGPU device: {error}");
            return None;
        }
        Err(error) => panic!("a WebGPU device: {error}"),
    };
    let target = RenderTarget::Offscreen { size, device: Some(device) };
    let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
    result_of(attached).unwrap();
    Some(session)
}

fn center_pixel(session: &RenderSession) -> [u8; 4] {
    let image = wait(session.read_texture());
    let offset = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

/// The render errors the map reported so far.
fn render_errors(log: &EventLog) -> Vec<String> {
    let errors = log.events().into_iter().filter_map(|event| match event.kind {
        RuntimeEventKind::Map(MapEvent::RenderError(error)) => Some(error.message),
        _ => None,
    });
    errors.collect()
}

/// A still image whose frame fails ends with the GPU's error, once its
/// demand's result, with no update, is queued. A still asked for right
/// after draws once the device works again.
#[test]
fn a_still_image_whose_frame_fails_ends_with_the_error() {
    let runtime = runtime();
    let log = EventLog::install(&runtime);
    let Some((map, session, broken)) = broken_map(&runtime, MapMode::Static) else { return };
    wait(map.request_still_image());
    assert_eq!(RED, center_pixel(&session));
    session.drain_frame_results().unwrap();

    broken.store(true, Ordering::SeqCst);
    let error = result_of(map.request_still_image().unwrap()).expect_err("a still whose frame failed");
    assert_eq!(ErrorKind::Native, error.kind);
    assert!(error.message.contains("Submitting the frame failed"), "{error}");
    let results = session.drain_frame_results().unwrap();
    let still = |result: &FrameResult| result.token.is_none();
    assert!(
        matches!(&results[..], [result] if still(result) && result.disposition == FrameDisposition::NoUpdate),
        "{results:?}"
    );
    assert_eq!(1, render_errors(&log).len());

    broken.store(false, Ordering::SeqCst);
    wait(map.request_still_image());
    assert_eq!(RED, center_pixel(&session));
}

/// A still image whose offscreen target can't make its textures, here for
/// a width that no WebGPU device takes, ends with an error, its demand's
/// result with no update, rather than waiting for ever.
#[test]
fn a_still_image_whose_target_cant_make_its_textures_ends_with_the_error() {
    let runtime = runtime();
    let size = Size::new(1 << 17, 1);
    let map = red_map(&runtime, MapMode::Static, size);
    let Some(session) = offscreen_session(&map, size) else { return };
    let error = result_of(map.request_still_image().unwrap()).expect_err("a still of a target without textures");
    assert_eq!(ErrorKind::InvalidState, error.kind, "{error}");
    let results = session.drain_frame_results().unwrap();
    assert!(
        matches!(&results[..], [result] if result.token.is_none() && result.disposition == FrameDisposition::NoUpdate),
        "{results:?}"
    );
}

/// The extent of a target whose logical size rounds down to no physical
/// pixels at its scale factor.
const NO_PIXELS: LogicalExtent = LogicalExtent { size: Size { width: 1, height: 64 }, scale_factor: 0.5 };

/// A still image of an offscreen target without pixels ends with an error
/// that says so, its demand's result with no update, rather than waiting
/// for ever. The GPU had nothing to do with it, so the map reports no
/// render error.
#[test]
fn a_still_image_of_a_target_without_pixels_ends_with_an_error() {
    let runtime = runtime();
    let log = EventLog::install(&runtime);
    let map = red_map(&runtime, MapMode::Static, SIZE);
    let Some(session) = offscreen_session(&map, SIZE) else { return };
    wait(session.resize(NO_PIXELS));
    let error = result_of(map.request_still_image().unwrap()).expect_err("a still of a target without pixels");
    assert_eq!(ErrorKind::InvalidState, error.kind, "{error}");
    assert!(error.message.contains("no pixels"), "{error}");
    let results = session.drain_frame_results().unwrap();
    assert!(
        matches!(&results[..], [result] if result.token.is_none() && result.disposition == FrameDisposition::NoUpdate),
        "{results:?}"
    );
    assert!(render_errors(&log).is_empty(), "{:?}", render_errors(&log));
}

/// A continuous map's demands on an offscreen target without pixels end as
/// not ready, with no render error, so the map doesn't draw again on the
/// failure backoff.
#[test]
fn a_continuous_map_on_a_target_without_pixels_isnt_ready() {
    let runtime = runtime();
    let log = EventLog::install(&runtime);
    let map = red_map(&runtime, MapMode::Continuous, SIZE);
    let Some(session) = offscreen_session(&map, SIZE) else { return };
    wait(session.resize(NO_PIXELS));
    session.drain_frame_results().unwrap();
    for _ in 0..3 {
        session.demand_frame(FrameDemand::new(DemandMode::Force, 0)).unwrap();
        let mut results = Vec::new();
        assert!(eventually(|| {
            results.extend(session.drain_frame_results().unwrap());
            !results.is_empty()
        }));
        assert!(results.iter().all(|result| result.disposition == FrameDisposition::TargetNotReady), "{results:?}");
    }
    assert!(render_errors(&log).is_empty(), "{:?}", render_errors(&log));
}

/// A continuous map whose frames keep failing reports it once, draws again
/// once on its own, and goes idle with everything loaded, its frames
/// rendered with what did draw. Once the device works again, the next frame
/// that anything causes draws the map.
#[test]
fn a_continuous_map_whose_frames_fail_reports_once_and_draws_again_once() {
    let runtime = runtime();
    let log = EventLog::install(&runtime);
    let Some((map, session, broken)) = broken_map(&runtime, MapMode::Continuous) else { return };
    // Demands a frame of each update, and returns the results of `within`.
    let frames = |within: Duration| {
        let mut results: Vec<FrameResult> = Vec::new();
        let start = Instant::now();
        while start.elapsed() < within {
            session.demand_frame(FrameDemand::new(DemandMode::WaitForUpdate, 0)).unwrap();
            let mut taken = Vec::new();
            eventually(|| {
                taken.extend(session.drain_frame_results().unwrap());
                !taken.is_empty() || start.elapsed() >= within
            });
            results.extend(taken);
        }
        results
    };
    session.demand_frame(FrameDemand::new(DemandMode::Force, 0)).unwrap();
    assert!(eventually(|| session.drain_frame_results().unwrap().iter().any(|result| result.mode == RenderMode::Full)));

    broken.store(true, Ordering::SeqCst);
    wait(map.request_repaint());
    let failing = frames(Duration::from_secs(1));
    let drawn = failing.iter().filter(|result| result.disposition == FrameDisposition::Rendered).count();
    assert_eq!(2, drawn, "the failed frame and the one that drew it again: {failing:?}");
    assert_eq!(1, render_errors(&log).len(), "{:?}", render_errors(&log));
    assert!(map.snapshot().unwrap().fully_loaded);

    broken.store(false, Ordering::SeqCst);
    wait(map.request_repaint());
    let recovered = frames(Duration::from_millis(500));
    assert!(
        recovered
            .iter()
            .any(|result| result.disposition == FrameDisposition::Rendered && result.mode == RenderMode::Full),
        "{recovered:?}"
    );
    assert_eq!(RED, center_pixel(&session));
    assert_eq!(1, render_errors(&log).len());
}
