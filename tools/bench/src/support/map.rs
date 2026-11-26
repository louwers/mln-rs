//! A map drawn by a headless frontend on the calling thread, which the
//! scenario drives one step at a time: frames on a frame clock of its own,
//! and waits for the map's background work to finish, so the frames a
//! scenario draws and what each has loaded are the same on every run and
//! every machine.

use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub(crate) use mln_bench_workload::paths::{tile_at, tile_center};
use mln_engine::RenderMode;
use mln_engine::camera::CameraOptions;
use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapEvent, MapMode, MapOptions, RenderFrameStatus};
use mln_engine::storage::FileSource;
use mln_headless::HeadlessFrontend;
use mln_runtime::{BackendRef, Device};
use mln_style::size::Size;
use mln_style::sources::geojson::{GeoJSONOptions, GeoJsonSourceType, create_geojson_data, geojson_source};
use mln_webgpu::bench::PipelineStatistics;

use super::fixtures::Fixtures;
use crate::environment::Gpu;
use crate::scenario::Bench;

/// The interval of the frame clock: a 60 Hz display's.
const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);

/// How many frames [`HeadlessMap::settle`] and [`HeadlessMap::load`] may
/// draw: a guard against a map that never completes.
const FRAME_LIMIT: u64 = 10_000;

/// The GPU device and the offscreen target a map draws into, with the
/// queue and the pool the map runs on. Making one is the part of a map's
/// start that a scenario leaves out.
///
/// The pool has no threads: the map's background work (file reads, tile
/// layout, glyph and sprite parsing, GeoJSON slicing) runs on the calling
/// thread when the scenario waits for it, one task after another in the
/// same order on every run, as in the browser.
pub(crate) struct Frontend {
    /// The frontend's queue, on the calling thread.
    queue: Queue<()>,
    pool: Pool,
    /// The times work arrived at the pool while none was queued.
    arrivals: Arc<AtomicU64>,
    /// What the GPU's pipeline has run for the frames so far, where the
    /// device counts it.
    pipeline: Cell<Option<PipelineStatistics>>,
    frontend: HeadlessFrontend,
}

impl Frontend {
    /// A frontend of `size` at pixel ratio 1, on the device the tests
    /// render on: the backend `MLN_RENDER_BACKEND` names, or WebGPU.
    pub(crate) fn new(size: Size) -> Frontend {
        let queue = Queue::new();
        let pool = Pool::local();
        let arrivals = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&arrivals);
        pool.set_waker(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        })));
        let frontend = HeadlessFrontend::new(size, 1.0, &queue, &pool).expect("a headless frontend");
        Frontend { queue, pool, arrivals, pipeline: Cell::new(None), frontend }
    }

    /// The pool a file source for the map should run its reads on, so that
    /// waiting for the map's work runs them too.
    pub(crate) fn pool(&self) -> &Pool {
        &self.pool
    }

    /// The device the frontend draws on, which other maps' render sessions
    /// can share.
    pub(crate) fn device(&self) -> Device {
        self.frontend.device().clone()
    }

    /// The device the frontend draws on, described.
    pub(crate) fn gpu(&self) -> Gpu {
        match self.frontend.backend() {
            #[cfg(target_vendor = "apple")]
            BackendRef::Metal(metal) => {
                Gpu { name: metal.device().name(), backend: "Metal".to_owned(), software: false }
            }
            #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
            BackendRef::Vulkan(vulkan) => Gpu {
                name: vulkan.get_device_name(),
                backend: "Vulkan".to_owned(),
                software: vulkan.is_software_device(),
            },
            #[cfg(windows)]
            BackendRef::D3d12(d3d12) => Gpu {
                name: d3d12.device().adapter_name().to_owned(),
                backend: "D3D12".to_owned(),
                software: d3d12.device().is_software(),
            },
            BackendRef::Gl(gl) => {
                let name = gl.get_renderer_name();
                let lower = name.to_lowercase();
                let software = ["llvmpipe", "softpipe", "swiftshader"].iter().any(|s| lower.contains(s));
                Gpu { name, backend: "OpenGL".to_owned(), software }
            }
            BackendRef::WebGpu(webgpu) => {
                let info = webgpu.device().adapter_info();
                let lower = info.name.to_lowercase();
                let software = info.device_type == wgpu::DeviceType::Cpu
                    || ["llvmpipe", "lavapipe", "swiftshader", "software", "warp"].iter().any(|s| lower.contains(s));
                Gpu { name: info.name, backend: format!("WebGPU on {:?}", info.backend), software }
            }
            _ => panic!("a device of no known backend"),
        }
    }

    /// Waits for the GPU to finish the frame drawn last: the native backends
    /// submit it without waiting, and WebGPU leaves it to the device.
    fn wait_for_gpu(&mut self) {
        let Device::WebGpu(device) = self.frontend.device() else {
            let mut flushed = Ok(());
            self.frontend.target().with_gpu(|gpu| flushed = gpu.flush(true));
            flushed.expect("the GPU finishes the frame");
            return;
        };
        device.device().poll(wgpu::PollType::wait_indefinitely()).expect("the GPU finishes the frame");
        if let Some(ran) = mln_webgpu::bench::take_pipeline_statistics() {
            let total = self.pipeline.get().unwrap_or_default();
            self.pipeline.set(Some(PipelineStatistics {
                vertex_shader_invocations: total.vertex_shader_invocations + ran.vertex_shader_invocations,
                primitives: total.primitives + ran.primitives,
                fragment_shader_invocations: total.fragment_shader_invocations + ran.fragment_shader_invocations,
            }));
        }
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
    /// [`background_work`](Self::background_work) when the pool was last
    /// emptied.
    drained: u64,
}

impl HeadlessMap {
    /// A map in `mode`, the frontend's size, whose resources come from
    /// `file_source`, attached to the frontend. Make `file_source` run on
    /// [`Frontend::pool`].
    pub(crate) fn new(mut frontend: Frontend, mode: MapMode, file_source: Arc<dyn FileSource>) -> HeadlessMap {
        let size = frontend.frontend.size();
        let options = MapOptions { map_mode: mode, size, pixel_ratio: 1.0, ..Default::default() };
        let mut map = Map::with_file_source(file_source, options, &frontend.pool).expect("a map");
        frontend.frontend.attach(&mut map);
        let clock = Instant::now() + Duration::from_hours(24);
        HeadlessMap { map, frontend, clock, frames: 0, incomplete: 0, last_frame: None, drained: 0 }
    }

    /// A map as [`new`](Self::new) makes, over the fixtures, on a new
    /// frontend of `size`, at `camera`, loading `style`, such as
    /// [`Snapshot::style`](mln_bench_workload::Snapshot::style). Records the frontend's device in `bench`.
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
        let file_source = fixtures.file_source(frontend.pool());
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

    /// What the GPU's pipeline has run for the map's frames so far, where
    /// the device counts it: on WebGPU on Vulkan or D3D12.
    pub(crate) fn pipeline_statistics(&self) -> Option<PipelineStatistics> {
        self.frontend.pipeline.get()
    }

    /// Declares in `bench` what the GPU's pipeline ran since it ran `since`,
    /// where the device counts it: `gpu_vertex_shader_invocations`,
    /// `gpu_primitives` (those its clipper passed on) and
    /// `gpu_fragment_shader_invocations`.
    pub(crate) fn count_pipeline(&self, bench: &mut Bench, since: Option<PipelineStatistics>) {
        let (Some(now), Some(since)) = (self.pipeline_statistics(), since) else { return };
        bench.count("gpu_vertex_shader_invocations", now.vertex_shader_invocations - since.vertex_shader_invocations);
        bench.count("gpu_primitives", now.primitives - since.primitives);
        bench.count(
            "gpu_fragment_shader_invocations",
            now.fragment_shader_invocations - since.fragment_shader_invocations,
        );
    }

    /// How many times background work has arrived at the map's idle pool
    /// so far: unchanged while the map starts none.
    pub(crate) fn background_work(&self) -> u64 {
        self.frontend.arrivals.load(Ordering::Relaxed)
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
        let mut any = self.background_work() != self.drained;
        loop {
            self.frontend.pool.run_all();
            // The pool is empty now, so any task spawned from here on
            // arrives at an idle pool, and counts.
            self.drained = self.background_work();
            let mut ran = self.frontend.queue.run_once(&mut ());
            // The map's handling of what the workers sent can start more
            // work, such as the requests of a style that loaded.
            if self.map.has_reports() {
                self.map.process_events();
                ran = true;
            }
            if !ran && self.background_work() == self.drained {
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
        self.frontend.queue.run_once(&mut ());
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
    /// fading in. When the map changes next on its own, as when a symbol
    /// placement is due, the clock skips to the frame then, as a host's
    /// display would draw nothing until it. Returns how many it drew.
    pub(crate) fn settle(&mut self) -> u64 {
        let start = self.frames;
        loop {
            self.wait_for_workers();
            let status = self.draw_frame();
            if status.mode == RenderMode::Full {
                if let Some(at) = self.map.repaint_at() {
                    while self.clock < at {
                        self.clock += FRAME_INTERVAL;
                    }
                    continue;
                }
                if !status.needs_repaint {
                    return self.frames - start;
                }
            }
            assert!(self.frames - start < FRAME_LIMIT, "the view isn't settled after {FRAME_LIMIT} frames");
        }
    }

    /// Renders a still image of a static or tile map, waiting for the
    /// workers between the renderer's updates, and reads it back from the
    /// GPU.
    pub(crate) fn render_still(&mut self) -> mln_style::bitmap::PremultipliedImage {
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
    /// tiles are cut on the renderer's pool, the map's.
    pub(crate) fn set_geojson(&mut self, id: &str, json: &str) {
        let geojson = mln_runtime::bench::parse_geojson(json).expect("GeoJSON");
        let data = create_geojson_data(geojson, &GeoJSONOptions::default_options()).expect("GeoJSON that tiles");
        self.map
            .edit_style(|style| style.apply(GeoJsonSourceType::set_data(id, Some(data))))
            .expect("a GeoJSON source");
    }

    /// The vertex and index buffers the GPU held after the last frame.
    pub(crate) fn gpu_buffer_bytes(&self) -> u64 {
        let stats = self.frontend.frontend.rendering_stats();
        (i64::from(stats.mem_vertex_buffers) + i64::from(stats.mem_index_buffers)) as u64
    }

    /// The draw calls the renderer has made so far, those into terrain's
    /// drapes included.
    pub(crate) fn draw_calls(&self) -> u64 {
        self.frontend.frontend.rendering_stats().total_draw_calls as u64
    }

    /// The textures and buffers the renderer has made on the GPU so far.
    pub(crate) fn gpu_objects_created(&self) -> u64 {
        let stats = self.frontend.frontend.rendering_stats();
        stats.num_created_textures as u64 + stats.total_buffer_objs as u64
    }

    /// The last frame, read back from the GPU.
    pub(crate) fn image(&mut self) -> mln_style::bitmap::PremultipliedImage {
        self.frontend.frontend.read_still_image()
    }

    /// Writes the last frame as a PNG to `path`.
    pub(crate) fn save_image(&mut self, path: &Path) {
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

/// What a map's 3D terrain drew and held over the frames a scenario
/// measured, from the rendering stats after each
/// ([`frame`](Self::frame)).
#[derive(Default)]
pub(crate) struct Drapes {
    drawn: u64,
    most: u64,
    most_bytes: u64,
}

impl Drapes {
    /// Adds what `map`'s last frame drew of its drapes, and what they hold.
    pub(crate) fn frame(&mut self, map: &HeadlessMap) {
        let terrain = map.frontend.frontend.rendering_stats().terrain;
        self.drawn += terrain.drapes_drawn as u64;
        self.most = self.most.max(terrain.drapes as u64);
        self.most_bytes = self.most_bytes.max(terrain.mem_drapes as u64);
    }

    /// Declares in `bench`, where `map`'s style has terrain, the drapes the
    /// frames drew, `drapes_drawn`; the most drapes held after any of them,
    /// `drapes`; and the most bytes their textures held, `gpu_drapes`.
    pub(crate) fn declare(&self, bench: &mut Bench, map: &HeadlessMap) {
        if map.map.style().get_terrain().is_none() {
            return;
        }
        bench.count("drapes_drawn", self.drawn);
        bench.count("drapes", self.most);
        bench.bytes("gpu_drapes", self.most_bytes);
    }
}

/// Writes `image` as a PNG to `path`.
pub(crate) fn save_png(image: &mln_style::bitmap::PremultipliedImage, path: &Path) {
    std::fs::write(path, mln_engine::image::encode_png(image))
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}
