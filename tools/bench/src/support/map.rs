//! A continuous map drawn by a headless frontend on the calling thread,
//! whose frames the scenario asks for one at a time: no display clock and no
//! sleeping, so the frames a scenario draws are the same on every machine.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_actor::RunLoop;
use mln_headless::HeadlessFrontend;
use mln_map::{CameraOptions, Map, MapEvent, MapMode, MapOptions, RenderFrameStatus, RenderMode};
use mln_runtime::Device;
use mln_storage::FileSource;
use mln_util::size::Size;

use crate::environment::Gpu;

/// How long [`HeadlessMap::settle`] may wait before it fails the scenario:
/// a guard against a hang, not part of what runs.
const SETTLE_LIMIT: Duration = Duration::from_secs(120);

pub(crate) struct HeadlessMap {
    /// The loop of the map and the frontend, on the calling thread.
    run_loop: RunLoop,
    frontend: HeadlessFrontend,
    map: Map,
    frames: u64,
    last_frame: Option<RenderFrameStatus>,
}

impl HeadlessMap {
    /// A continuous map of `size` without a style, on the GPU device the
    /// headless frontend picks (`MLN_RENDER_BACKEND`, WebGPU by default).
    pub(crate) fn new(size: Size) -> HeadlessMap {
        HeadlessMap::build(size, None)
    }

    /// A map as [`HeadlessMap::new`] makes, whose resources come from
    /// `file_source`, such as the fixtures'
    /// ([`Fixtures::file_source`](super::fixtures::Fixtures::file_source)).
    #[expect(dead_code, reason = "no scenario reads the fixtures")]
    pub(crate) fn with_file_source(size: Size, file_source: Arc<dyn FileSource>) -> HeadlessMap {
        HeadlessMap::build(size, Some(file_source))
    }

    fn build(size: Size, file_source: Option<Arc<dyn FileSource>>) -> HeadlessMap {
        let run_loop = RunLoop::new();
        let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop).expect("a headless frontend");
        let options = MapOptions { map_mode: MapMode::Continuous, size, pixel_ratio: 1.0, ..Default::default() };
        let map = match file_source {
            Some(file_source) => Map::with_file_source(file_source, options, &run_loop),
            None => Map::new(options, &run_loop),
        }
        .expect("a map");
        frontend.attach(&map);
        HeadlessMap { run_loop, frontend, map, frames: 0, last_frame: None }
    }

    pub(crate) fn map(&mut self) -> &mut Map {
        &mut self.map
    }

    pub(crate) fn jump(&mut self, camera: &CameraOptions) {
        self.map.jump_to(camera);
    }

    /// The frames drawn so far.
    pub(crate) fn frames(&self) -> u64 {
        self.frames
    }

    /// Draws and waits for the map's work, such as its tiles from the
    /// workers, until a frame shows the view complete and nothing is left to
    /// animate.
    pub(crate) fn settle(&mut self) {
        let start = Instant::now();
        loop {
            // The map's events can publish an update, which the frontend
            // then draws.
            self.observe();
            let drew = self.frontend.render_update();
            if drew {
                self.wait_for_gpu();
            }
            if self.run_loop.run_once() || drew {
                continue;
            }
            if self.last_frame.as_ref().is_some_and(|status| status.mode == RenderMode::Full && !status.needs_repaint) {
                return;
            }
            assert!(start.elapsed() < SETTLE_LIMIT, "the view isn't complete after {SETTLE_LIMIT:?}");
            self.run_loop.wait_for_work();
        }
    }

    /// Runs what is ready without waiting for more, then draws the map's
    /// latest update and waits for the GPU to finish it, as a host does
    /// before presenting a frame. Returns how the frame went.
    pub(crate) fn draw_frame(&mut self) -> RenderFrameStatus {
        while self.run_loop.run_once() {}
        self.map.process_events();
        self.frontend.render_frame();
        self.wait_for_gpu();
        self.observe();
        self.last_frame.clone().expect("the renderer reports each frame")
    }

    /// The vertex and index buffers the GPU held after the last frame.
    pub(crate) fn gpu_buffer_bytes(&self) -> u64 {
        let stats = &self.last_frame.as_ref().expect("a frame").rendering_stats;
        (i64::from(stats.mem_vertex_buffers) + i64::from(stats.mem_index_buffers)) as u64
    }

    /// Handles the map's events, counting its frames.
    fn observe(&mut self) {
        for event in self.map.drain_events() {
            match event {
                MapEvent::RenderFrameFinished(status) => {
                    self.frames += 1;
                    self.last_frame = Some(status);
                }
                MapEvent::LoadingFailed(error) => panic!("the map failed to load: {error}"),
                _ => {}
            }
        }
    }

    /// Waits for the GPU to finish the frame drawn last: the native backends
    /// submit it without waiting, and WebGPU leaves it to the device.
    fn wait_for_gpu(&self) {
        let Device::WebGpu(device) = self.frontend.device() else {
            let backend = self.frontend.backend();
            let _hold = backend.hold();
            backend.gpu().flush(true).expect("the GPU finishes the frame");
            return;
        };
        device.device().poll(wgpu::PollType::wait_indefinitely()).expect("the GPU finishes the frame");
    }

    /// The device the map draws on.
    pub(crate) fn gpu(&self) -> Gpu {
        let backend = self.frontend.backend();
        #[cfg(target_vendor = "apple")]
        if let Device::Metal(metal) = self.frontend.device() {
            return Gpu { name: metal.name(), backend: "Metal".to_owned(), software: false };
        }
        #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
        if let Some(vulkan) = backend.downcast_ref::<mln_vulkan::RendererBackend>() {
            return Gpu {
                name: vulkan.get_device_name(),
                backend: "Vulkan".to_owned(),
                software: vulkan.is_software_device(),
            };
        }
        #[cfg(windows)]
        if let Some(d3d12) = backend.downcast_ref::<mln_d3d12::RendererBackend>() {
            let gpu = d3d12.d3d12_gpu();
            return Gpu {
                name: gpu.device().adapter_name().to_owned(),
                backend: "D3D12".to_owned(),
                software: gpu.device().is_software(),
            };
        }
        if let Some(gl) = backend.downcast_ref::<mln_gl::RendererBackend>() {
            let name = gl.get_renderer_name();
            let lower = name.to_lowercase();
            let software = ["llvmpipe", "softpipe", "swiftshader"].iter().any(|s| lower.contains(s));
            return Gpu { name, backend: "OpenGL".to_owned(), software };
        }
        let Device::WebGpu(device) = self.frontend.device() else {
            panic!("a device of no known backend");
        };
        let info = device.adapter_info();
        let lower = info.name.to_lowercase();
        let software = info.device_type == wgpu::DeviceType::Cpu
            || ["llvmpipe", "lavapipe", "swiftshader", "software", "warp"].iter().any(|s| lower.contains(s));
        Gpu { name: info.name, backend: format!("WebGPU on {:?}", info.backend), software }
    }
}
