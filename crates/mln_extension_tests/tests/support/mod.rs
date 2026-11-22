//! A runtime of a registry with the extension types, a map of `SIZE` and an
//! offscreen session of it on WebGPU, through the facade.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_rs::mln_render::Registry;
use mln_rs::mln_util::size::Size;
use mln_rs::{
    AttachOptions, BackendType, Completion, DemandMode, Device, DeviceOptions, Error, FrameDemand, FrameDisposition,
    Map, MapOptions, RenderSession, RenderTarget, Runtime, RuntimeOptions,
};

pub(crate) const SIZE: Size = Size { width: 64, height: 64 };

/// A runtime of a registry, a map of `SIZE` with a style, and an offscreen
/// session of it that shows the style loaded. The session draws with
/// WebGPU, whatever the build's default backend, since the extension types'
/// programs have WGSL alone.
pub(crate) struct Fixture {
    pub(crate) session: RenderSession,
    pub(crate) map: Map,
    runtime: Runtime,
}

impl Fixture {
    pub(crate) fn new(registry: Arc<Registry>, style: &str) -> Fixture {
        let runtime = Runtime::new(&RuntimeOptions::default(), registry).unwrap();
        let options =
            MapOptions { map: mln_rs::map::MapCoreOptions { size: SIZE, ..Default::default() }, ..Default::default() };
        let (map, created) = runtime.create_map(options).unwrap();
        created.wait().unwrap();
        wait(map.set_style_json(style));
        let device = Device::with_backend(BackendType::WebGPU, DeviceOptions::default()).unwrap();
        let target = RenderTarget::Offscreen { size: SIZE, device: Some(device) };
        let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
        attached.wait().unwrap();
        let fixture = Fixture { session, map, runtime };
        fixture.render_loaded();
        fixture
    }

    /// Renders one frame, whether or not the map changed.
    pub(crate) fn render(&self) -> mln_rs::FrameResult {
        self.session.demand_frame(FrameDemand::new(DemandMode::Force, 0)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(result) = self.session.drain_frame_results().unwrap().into_iter().next() {
                assert_eq!(FrameDisposition::Rendered, result.disposition);
                return result;
            }
            assert!(Instant::now() < deadline, "no frame rendered");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Renders until a frame shows every source loaded.
    pub(crate) fn render_loaded(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while {
            let frame = self.render();
            frame.mode != mln_rs::mln_render::RenderMode::Full || frame.needs_repaint
        } {
            assert!(Instant::now() < deadline, "the map never loaded");
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        wait(self.session.release());
        wait(self.map.release());
        if let Ok(released) = self.runtime.release() {
            let _ = released.wait();
        }
    }
}

pub(crate) fn wait<T: Send + 'static>(completion: Result<Completion<T>, Error>) -> T {
    completion.unwrap().wait().unwrap()
}
