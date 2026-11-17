//! A map drawn by a headless frontend on the calling thread, which the
//! scenario drives one step at a time: frames on a frame clock of its own,
//! and waits for the map's background work to finish, so the frames a
//! scenario draws and what each has loaded are the same on every run and
//! every machine.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_actor::RunLoop;
use mln_actor::scheduler::SchedulerPtr;
use mln_headless::HeadlessFrontend;
use mln_map::{CameraOptions, Map, MapEvent, MapMode, MapOptions, RenderFrameStatus, RenderMode};
use mln_runtime::Device;
use mln_storage::FileSource;
use mln_style::sources::geojson::{GeoJSONOptions, GeoJsonSourceType, create_geojson_data, geojson_source};
use mln_util::geo::LatLng;
use mln_util::size::Size;

use super::fixtures::Fixtures;
use super::workers::Workers;
use crate::environment::Gpu;
use crate::scenario::Bench;

/// The interval of the frame clock: a 60 Hz display's.
const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);

/// How many frames [`HeadlessMap::settle`] and [`HeadlessMap::load`] may
/// draw: a guard against a map that never completes.
const FRAME_LIMIT: u64 = 10_000;

/// The GPU device and the offscreen target a map draws into, with the run
/// loop and the workers the map runs on. Making one is the part of a map's
/// start that a scenario leaves out.
pub(crate) struct Frontend {
    /// The loop of the map and the frontend, on the calling thread.
    run_loop: RunLoop,
    workers: Arc<Workers>,
    frontend: HeadlessFrontend,
}

impl Frontend {
    /// A frontend of `size` at pixel ratio 1, on the device the tests
    /// render on: the backend `MLN_RENDER_BACKEND` names, or WebGPU.
    pub(crate) fn new(size: Size) -> Frontend {
        let workers = Workers::new();
        let pool: SchedulerPtr = Arc::<Workers>::clone(&workers);
        let run_loop = RunLoop::with_pool(pool);
        let frontend = HeadlessFrontend::new(size, 1.0, &run_loop).expect("a headless frontend");
        Frontend { run_loop, workers, frontend }
    }

    /// Where a file source for the map should run its reads, so that
    /// waiting for the workers waits for them too.
    pub(crate) fn workers(&self) -> SchedulerPtr {
        Arc::<Workers>::clone(&self.workers)
    }

    /// The device the frontend draws on, which other maps' render sessions
    /// can share.
    pub(crate) fn device(&self) -> Device {
        self.frontend.device().clone()
    }

    /// The device the frontend draws on, described.
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
}

/// A map on a [`Frontend`], over the fixtures or another file source.
///
/// Nothing runs on its own: the scenario draws each frame
/// ([`draw_frame`](Self::draw_frame)) and waits for the map's background
/// work ([`wait_for_workers`](Self::wait_for_workers)), or does both until
/// the map is loaded ([`load`](Self::load), [`settle`](Self::settle),
/// [`render_still`](Self::render_still)). A frame drawn after a wait has
/// everything the frames before it asked for, so a map loads in the same
/// steps on every run. Each continuous frame is one tick of a frame clock
/// that only frames advance, which places and fades symbols and runs
/// transitions as a 60 Hz display would, however long a frame took.
pub(crate) struct HeadlessMap {
    // The map before its frontend, whose renderer reports to it.
    map: Map,
    frontend: Frontend,
    /// The time of the next frame. It starts a day ahead of the system's
    /// clock, so the map never takes it for a stale frame time and always
    /// samples its animations at it.
    clock: Instant,
    frames: u64,
    incomplete: u64,
    last_frame: Option<RenderFrameStatus>,
}

impl HeadlessMap {
    /// A map in `mode`, the frontend's size, whose resources come from
    /// `file_source`, attached to the frontend. Make `file_source` run on
    /// [`Frontend::workers`].
    pub(crate) fn new(mut frontend: Frontend, mode: MapMode, file_source: Arc<dyn FileSource>) -> HeadlessMap {
        let size = frontend.frontend.size();
        let options = MapOptions { map_mode: mode, size, pixel_ratio: 1.0, ..Default::default() };
        let map = Map::with_file_source(file_source, options, &frontend.run_loop).expect("a map");
        frontend.frontend.attach(&map);
        let clock = Instant::now() + Duration::from_hours(24);
        HeadlessMap { map, frontend, clock, frames: 0, incomplete: 0, last_frame: None }
    }

    /// A map as [`new`](Self::new) makes, over the fixtures, on a new
    /// frontend of `size`, at `camera`, loading `style`, such as
    /// [`Fixtures::style`]. Records the frontend's device in `bench`.
    pub(crate) fn open(
        bench: &mut Bench,
        fixtures: &Fixtures,
        size: Size,
        mode: MapMode,
        style: &str,
        camera: &CameraOptions,
    ) -> HeadlessMap {
        let frontend = Frontend::new(size);
        bench.gpu = Some(frontend.gpu());
        let file_source = fixtures.file_source(frontend.workers());
        let mut map = HeadlessMap::new(frontend, mode, file_source);
        map.jump(camera);
        map.map().load_style_json(style);
        map
    }

    pub(crate) fn map(&mut self) -> &mut Map {
        &mut self.map
    }

    pub(crate) fn jump(&mut self, camera: &CameraOptions) {
        self.map.jump_to(camera);
    }

    /// The background tasks the map has started so far.
    pub(crate) fn background_tasks(&self) -> u64 {
        self.frontend.workers.started()
    }

    /// The frames drawn so far.
    pub(crate) fn frames(&self) -> u64 {
        self.frames
    }

    /// The frames drawn so far with something the view needs missing.
    pub(crate) fn frames_incomplete(&self) -> u64 {
        self.incomplete
    }

    /// Runs the map's background work, and what it sends back to the map
    /// and its renderer, until none is left. Returns whether there was any.
    pub(crate) fn wait_for_workers(&mut self) -> bool {
        let mut any = false;
        loop {
            self.frontend.workers.wait_until_idle();
            let mut ran = false;
            while self.frontend.run_loop.run_once() {
                ran = true;
            }
            // The map's handling of what the workers sent can start more
            // work, such as the requests of a style that loaded.
            if self.map.has_reports() {
                self.map.process_events();
                ran = true;
            }
            if !ran && self.frontend.workers.is_idle() {
                self.observe();
                return any;
            }
            any = true;
        }
    }

    /// Draws a frame of a continuous map at the next tick of its frame
    /// clock, and waits for the GPU to finish it, as a host does before
    /// presenting a frame. Runs what is ready on the map's run loop first,
    /// but doesn't wait for the workers. Returns how the frame went.
    pub(crate) fn draw_frame(&mut self) -> RenderFrameStatus {
        while self.frontend.run_loop.run_once() {}
        self.map.set_frame_time(Some(self.clock));
        self.clock += FRAME_INTERVAL;
        // An update at this frame's time, whether or not the map had one.
        self.map.trigger_repaint();
        self.map.process_events();
        let drawn = self.frames;
        self.frontend.frontend.render_frame();
        self.frontend.wait_for_gpu();
        self.observe();
        assert_eq!(self.frames, drawn + 1, "the renderer reports each frame");
        self.last_frame.clone().expect("a frame")
    }

    /// Draws frames, waiting for the workers before each, until one shows
    /// the view complete. Returns how many it drew.
    pub(crate) fn load(&mut self) -> u64 {
        let start = self.frames;
        loop {
            self.wait_for_workers();
            if self.draw_frame().mode == RenderMode::Full {
                return self.frames - start;
            }
            assert!(self.frames - start < FRAME_LIMIT, "the view isn't complete after {FRAME_LIMIT} frames");
        }
    }

    /// Draws frames, waiting for the workers before each, until one shows
    /// the view complete with nothing left to animate, such as symbols
    /// fading in. Returns how many it drew.
    pub(crate) fn settle(&mut self) -> u64 {
        let start = self.frames;
        loop {
            self.wait_for_workers();
            let status = self.draw_frame();
            if status.mode == RenderMode::Full && !status.needs_repaint {
                return self.frames - start;
            }
            assert!(self.frames - start < FRAME_LIMIT, "the view isn't settled after {FRAME_LIMIT} frames");
        }
    }

    /// Renders a still image of a static or tile map, waiting for the
    /// workers between the renderer's updates, and reads it back from the
    /// GPU.
    pub(crate) fn render_still(&mut self) -> mln_util::image::PremultipliedImage {
        self.map.render_still().expect("a still image starts");
        loop {
            self.map.process_events();
            if let Some(result) = self.map.take_still_image() {
                result.expect("a still image");
                return self.frontend.frontend.read_still_image();
            }
            let drew = self.frontend.frontend.render_update();
            let worked = self.wait_for_workers();
            assert!(drew || worked, "a still image that waits for nothing");
        }
    }

    /// Adds a GeoJSON source `id`, with the default options, holding the
    /// GeoJSON text `json`, as [`set_geojson`](Self::set_geojson) does.
    pub(crate) fn add_geojson_source(&mut self, id: &str, json: &str) {
        self.map.add_source(geojson_source(id, GeoJSONOptions::default_options())).expect("a new source");
        self.set_geojson(id, json);
    }

    /// Replaces the data of the GeoJSON source `id` with the GeoJSON text
    /// `json`, as an application's update through the runtime does: parses
    /// it, indexes it for tiling and gives it to the source. The source's
    /// tiles are cut on the map's workers, where a GeoJSON source otherwise
    /// cuts them on threads of its own, so that
    /// [`wait_for_workers`](Self::wait_for_workers) waits for them too.
    pub(crate) fn set_geojson(&mut self, id: &str, json: &str) {
        let geojson = mln_runtime::bench::parse_geojson(json).expect("GeoJSON");
        let data = create_geojson_data(geojson, self.frontend.workers(), &GeoJSONOptions::default_options())
            .expect("GeoJSON that tiles");
        self.map
            .edit_style(|style| style.apply(GeoJsonSourceType::set_data(id, Some(data))))
            .expect("a GeoJSON source");
    }

    /// The vertex and index buffers the GPU held after the last frame.
    pub(crate) fn gpu_buffer_bytes(&self) -> u64 {
        let stats = self.frontend.frontend.rendering_stats();
        (i64::from(stats.mem_vertex_buffers) + i64::from(stats.mem_index_buffers)) as u64
    }

    /// The textures and buffers the renderer has made on the GPU so far.
    pub(crate) fn gpu_objects_created(&self) -> u64 {
        let stats = self.frontend.frontend.rendering_stats();
        stats.num_created_textures as u64 + stats.total_buffer_objs as u64
    }

    /// The last frame, read back from the GPU.
    pub(crate) fn image(&self) -> mln_util::image::PremultipliedImage {
        self.frontend.frontend.read_still_image()
    }

    /// Writes the last frame as a PNG to `path`.
    pub(crate) fn save_image(&self, path: &Path) {
        save_png(&self.image(), path);
    }

    /// Handles the map's events, counting its frames.
    fn observe(&mut self) {
        for event in self.map.drain_events() {
            match event {
                MapEvent::RenderFrameFinished(status) => {
                    self.frames += 1;
                    if status.mode != RenderMode::Full {
                        self.incomplete += 1;
                    }
                    self.last_frame = Some(status);
                }
                MapEvent::LoadingFailed(error) => panic!("the map failed to load: {error}"),
                _ => {}
            }
        }
    }
}

/// Writes `image` as a PNG to `path`.
pub(crate) fn save_png(image: &mln_util::image::PremultipliedImage, path: &Path) {
    std::fs::write(path, mln_image::encode_png(image)).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}

/// The center of the 512-pixel tile `z`/`x`/`y` of Web Mercator.
pub(crate) fn tile_center(z: u8, x: u32, y: u32) -> LatLng {
    let tiles = f64::from(1u32 << z);
    let longitude = (f64::from(x) + 0.5) / tiles * 360.0 - 180.0;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(y) + 0.5) / tiles)).sinh().atan().to_degrees();
    LatLng::new(latitude, longitude).expect("a tile's center")
}

/// The tile of zoom `z` that holds `point`.
pub(crate) fn tile_at(z: u8, point: LatLng) -> (u32, u32) {
    let tiles = f64::from(1u32 << z);
    let x = (point.longitude() + 180.0) / 360.0 * tiles;
    let latitude = point.latitude().to_radians();
    let y = (1.0 - latitude.tan().asinh() / std::f64::consts::PI) / 2.0 * tiles;
    (x.floor() as u32, y.floor() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_holds_its_center() {
        for (z, x, y) in [(0, 0, 0), (14, 2620, 6332), (6, 10, 24)] {
            assert_eq!(tile_at(z, tile_center(z, x, y)), (x, y));
        }
    }
}
