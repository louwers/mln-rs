// Frames that fail on the GPU, through a session's own WebGPU device: a
// custom layer that records a draw that WebGPU rejects fails the frame's
// submit, as a device out of memory would fail it. A still image ends with
// the error, and a continuous map reports it once, shows what drew, and
// draws again later.

#![cfg(all(feature = "webgpu", not(target_family = "wasm")))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mln_map::{MapEvent, MapMode, RenderMode};
use mln_runtime::{
    AttachOptions, BackendType, DemandMode, Device, DeviceOptions, ErrorKind, FrameDemand, FrameDisposition,
    FrameResult, Map, MapOptions, RenderSession, RenderTarget, Runtime, RuntimeEventKind,
};
use mln_style::layers::{CustomLayerHost, CustomLayerRenderParameters};
use mln_util::size::Size;

use crate::support::{EventLog, eventually, result_of, runtime, wait};

const SIZE: Size = Size { width: 64, height: 64 };

const RED: [u8; 4] = [255, 0, 0, 255];

const STYLE: &str = r##"{"version":8,"transition":{"duration":0,"delay":0},"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"#ff0000"}}]}"##;

/// A custom layer that, while `broken`, records a draw without a pipeline,
/// which WebGPU rejects when the frame is submitted.
struct Breaking {
    broken: Arc<AtomicBool>,
}

impl CustomLayerHost for Breaking {
    fn render(&self, parameters: &mut CustomLayerRenderParameters<'_>) {
        if self.broken.load(Ordering::SeqCst)
            && let Some(lent) = parameters.pass.downcast_mut::<mln_webgpu::CustomLayerPass>()
        {
            lent.pass.draw(0..3, 0..1);
        }
    }
}

/// A map of `mode` with a red background and a [`Breaking`] layer, and an
/// offscreen session of `SIZE` on a WebGPU device; `None` where the
/// machine has no WebGPU device.
fn broken_map(runtime: &Runtime, mode: MapMode) -> Option<(Map, RenderSession, Arc<AtomicBool>)> {
    let device = match Device::with_backend(BackendType::WebGPU, DeviceOptions::default()) {
        Ok(device) => device,
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no WebGPU device: {error}");
            return None;
        }
        Err(error) => panic!("a WebGPU device: {error}"),
    };
    let options = MapOptions {
        map: mln_map::MapOptions { size: SIZE, map_mode: mode, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    result_of(created).unwrap();
    wait(map.set_style_json(STYLE));
    let broken = Arc::new(AtomicBool::new(false));
    wait(map.add_custom_layer("breaking", Arc::new(Breaking { broken: Arc::clone(&broken) }), None));
    let target = RenderTarget::Offscreen { size: SIZE, device: Some(device) };
    let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
    result_of(attached).unwrap();
    Some((map, session, broken))
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

/// A continuous map whose frames keep failing reports it once, isn't idle,
/// and draws again by itself only now and then, not frame after frame, each
/// frame rendered and partial. Once the device works again, the next frame
/// that anything causes draws the map, and it goes idle.
#[test]
fn a_continuous_map_whose_frames_fail_reports_once_and_draws_again_later() {
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
    let failing = frames(Duration::from_millis(1500));
    assert!(failing.iter().all(|result| result.mode == RenderMode::Partial), "{failing:?}");
    let drawn = failing.iter().filter(|result| result.disposition == FrameDisposition::Rendered).count();
    // 100, 200, 400 and 800 ms apart: four or five in 1.5 s.
    assert!((1..=8).contains(&drawn), "{drawn} frames in 1.5 s: {failing:?}");
    let errors = render_errors(&log);
    assert_eq!(1, errors.len(), "{errors:?}");
    assert!(!map.snapshot().unwrap().fully_loaded);

    // A failure that lasts stops retrying by itself, so something else
    // has to cause the frame.
    broken.store(false, Ordering::SeqCst);
    wait(map.request_repaint());
    let recovered = frames(Duration::from_millis(1500));
    assert!(
        recovered
            .iter()
            .any(|result| result.disposition == FrameDisposition::Rendered && result.mode == RenderMode::Full),
        "{recovered:?}"
    );
    assert_eq!(RED, center_pixel(&session));
    assert_eq!(1, render_errors(&log).len());
    assert!(log.wait_for(|event| matches!(event.kind, RuntimeEventKind::Map(MapEvent::Idle))).is_some());
}
