//! A runtime of a registry with the extension types, a map of `SIZE` and an
//! offscreen session of it, through the facade, on the backend that
//! `MLN_RENDER_BACKEND` names (`webgpu`, `metal`, `vulkan`, `d3d12` or `gl`,
//! OpenGL ES), or WebGPU.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_rs::layer::MapMode;
use mln_rs::mln_render::Registry;
use mln_rs::mln_util::size::Size;
use mln_rs::{
    AttachOptions, BackendType, Completion, DemandMode, Device, DeviceOptions, Error, FrameDemand, FrameDisposition,
    Map, MapOptions, RenderSession, RenderTarget, Runtime, RuntimeOptions,
};

pub(crate) const SIZE: Size = Size { width: 64, height: 64 };

/// A runtime of a registry, a map of `SIZE` with a style, and an offscreen
/// session of it that shows the style loaded.
pub(crate) struct Fixture {
    pub(crate) session: RenderSession,
    pub(crate) map: Map,
    runtime: Runtime,
}

impl Fixture {
    /// A fixture of `registry` and `style`.
    pub(crate) fn new(registry: Arc<Registry>, style: &str) -> Fixture {
        Fixture::with_mode(registry, style, MapMode::Continuous)
    }

    /// A fixture of `registry` and `style` whose map renders in `mode`. A
    /// map of a still mode renders nothing until the test requests a still
    /// image.
    pub(crate) fn with_mode(registry: Arc<Registry>, style: &str, mode: MapMode) -> Fixture {
        let runtime = Runtime::new(&RuntimeOptions::default(), registry).unwrap();
        let map = mln_rs::map::MapCoreOptions { size: SIZE, map_mode: mode, ..Default::default() };
        let options = MapOptions { map, ..Default::default() };
        let (map, created) = runtime.create_map(options).unwrap();
        created.wait().unwrap();
        wait(map.set_style_json(style));
        let device = Device::with_backend(backend(), DeviceOptions::default()).unwrap();
        let target = RenderTarget::Offscreen { size: SIZE, device: Some(device) };
        let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
        attached.wait().unwrap();
        let fixture = Fixture { session, map, runtime };
        if mode == MapMode::Continuous {
            fixture.render_loaded();
        }
        fixture
    }

    /// Renders one frame, whether or not the map changed.
    pub(crate) fn render(&self) -> mln_rs::FrameResult {
        self.render_on(DemandMode::Force)
    }

    /// Renders one frame on a demand of `mode`, which must draw one.
    pub(crate) fn render_on(&self, mode: DemandMode) -> mln_rs::FrameResult {
        self.session.demand_frame(FrameDemand::new(mode, 0)).unwrap();
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

/// The backend that `MLN_RENDER_BACKEND` names, or WebGPU.
fn backend() -> BackendType {
    match std::env::var("MLN_RENDER_BACKEND").map(|backend| backend.to_ascii_lowercase()).as_deref() {
        Ok("metal") => BackendType::Metal,
        Ok("vulkan") => BackendType::Vulkan,
        Ok("d3d12") => BackendType::D3D12,
        Ok("gl") => BackendType::OpenGL,
        Ok("webgpu") | Err(_) => BackendType::WebGPU,
        Ok(other) => panic!("MLN_RENDER_BACKEND names no backend: {other}"),
    }
}
