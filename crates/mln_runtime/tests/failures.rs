// Frames that fail on the GPU, through a session's own WebGPU device: a
// custom layer that records a draw that WebGPU rejects fails the frame's
// submit, as a device out of memory would fail it. Such a frame is never the
// host's: a still image ends with the error.

#![cfg(all(feature = "webgpu", not(target_family = "wasm")))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_map::{MapEvent, MapMode};
use mln_runtime::{
    AttachOptions, BackendType, Device, DeviceOptions, ErrorKind, FrameDisposition, FrameResult, Map, MapOptions,
    RenderSession, RenderTarget, Runtime, RuntimeEventKind,
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

/// A still image whose frame fails ends with the GPU's error, and the host
/// gets no frame of it: no rendered result, and nothing to read back. A
/// still asked for right after draws once the device works again.
#[test]
fn a_still_image_whose_frame_fails_ends_with_the_error_and_hands_off_nothing() {
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
    let mut results: Vec<FrameResult> = Vec::new();
    assert!(eventually(|| {
        results.extend(session.drain_frame_results().unwrap());
        !results.is_empty()
    }));
    assert!(results.iter().all(|result| result.disposition != FrameDisposition::Rendered), "{results:?}");
    let read = result_of(session.read_texture().unwrap()).expect_err("the failed frame was read");
    assert_eq!(ErrorKind::InvalidState, read.kind);
    assert_eq!(1, render_errors(&log).len());

    broken.store(false, Ordering::SeqCst);
    wait(map.request_still_image());
    assert_eq!(RED, center_pixel(&session));
}
