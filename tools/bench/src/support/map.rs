//! A map on the runtime, drawn by a render session on the calling thread,
//! which the scenario drives one step at a time: frames on a frame clock of
//! its own, and waits for the map's background work to finish, so the
//! frames a scenario draws and what each has loaded are the same on every
//! run and every machine.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub(crate) use mln_bench_workload::paths::{tile_at, tile_center};
use mln_engine::RenderMode;
use mln_engine::camera::{CameraOptions, CameraUpdate, CameraUpdateMode};
use mln_engine::exec::pool::Pool;
use mln_engine::map::{MapEvent, MapMode};
use mln_engine::storage::FileSource;
use mln_gfx::rendering_stats::RenderingStats;
use mln_runtime::{
    AttachOptions, BackendRef, BackendType, Completion, DemandMode, Device, DeviceOptions, Driver, DriverWork,
    EventMask, EventType, FeatureState, FeatureStateSelector, FrameDemand, FrameDisposition, GeoJsonSourceOptions,
    GlApi, LocalTarget, Map, MapOptions, RenderSession, RenderTarget, Runtime, RuntimeEventKind,
};
use mln_style::feature::Identifier;
use mln_style::geo::LatLngBounds;
use mln_style::image::Impl as StyleImage;
use mln_style::size::Size;
use mln_webgpu::bench::PipelineStatistics;
use parking_lot::{Condvar, Mutex};

use super::fixtures::Fixtures;
use crate::environment::Gpu;
use crate::scenario::Bench;

/// The interval of the frame clock: a 60 Hz display's.
const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);

/// How many frames [`HeadlessMap::settle`] and [`HeadlessMap::load`] may
/// draw: a guard against a map that never completes.
const FRAME_LIMIT: u64 = 10_000;

/// How long a wait for the runtime's thread may take before the scenario
/// fails: far longer than any command takes.
const WAIT_LIMIT: Duration = Duration::from_secs(120);

/// The device the tests render on: the backend that `MLN_RENDER_BACKEND`
/// names, or WebGPU. `gl` names OpenGL ES, and `gl-desktop` desktop OpenGL.
fn harness_device() -> Device {
    let mut options = DeviceOptions::default();
    let backend = match std::env::var("MLN_RENDER_BACKEND").map(|backend| backend.to_ascii_lowercase()).as_deref() {
        Ok("metal") => BackendType::Metal,
        Ok("vulkan") => BackendType::Vulkan,
        Ok("d3d12") => BackendType::D3D12,
        Ok("gl") => BackendType::OpenGL,
        Ok("gl-desktop") => {
            options.gl_api = GlApi::Gl;
            BackendType::OpenGL
        }
        Ok("webgpu") | Err(_) => BackendType::WebGPU,
        Ok(other) => panic!("MLN_RENDER_BACKEND names no backend: {other}"),
    };
    Device::with_backend(backend, options).expect("a device")
}

/// The GPU device a map draws on, and the pool its background work runs
/// on. Making the device is the part of a map's start that a scenario
/// leaves out.
///
/// The pool has no threads: the map's background work (file reads, tile
/// layout, glyph and sprite parsing, GeoJSON slicing) runs on the calling
/// thread when the scenario waits for it, one task after another in the
/// same order on every run, as in the browser.
pub(crate) struct Frontend {
    size: Size,
    device: Device,
    pool: Pool,
    /// The times work arrived at the pool while none was queued.
    arrivals: Arc<AtomicU64>,
    /// What the GPU's pipeline has run for the frames so far, where the
    /// device counts it.
    pipeline: std::cell::Cell<Option<PipelineStatistics>>,
    /// Wakes the scenario's thread when work arrives at the pool.
    signal: Arc<Signal>,
}

impl Frontend {
    /// A frontend of `size` at pixel ratio 1, on the device the tests
    /// render on: the backend `MLN_RENDER_BACKEND` names, or WebGPU.
    pub(crate) fn new(size: Size) -> Frontend {
        let pool = Pool::local();
        let arrivals = Arc::new(AtomicU64::new(0));
        let signal = Arc::new(Signal::default());
        let (counter, wake) = (Arc::clone(&arrivals), Arc::clone(&signal));
        pool.set_waker(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
            wake.wake();
        })));
        Frontend { size, device: harness_device(), pool, arrivals, pipeline: std::cell::Cell::new(None), signal }
    }

    /// The pool a file source for the map should run its reads on, so that
    /// waiting for the map's work runs them too.
    pub(crate) fn pool(&self) -> &Pool {
        &self.pool
    }

    /// The device the frontend draws on, which other maps' render sessions
    /// can share.
    pub(crate) fn device(&self) -> Device {
        self.device.clone()
    }

    /// The device the frontend draws on, described.
    pub(crate) fn gpu(&self) -> Gpu {
        // A target of the device's backend describes it.
        let target = LocalTarget::offscreen(Some(self.device.clone()), Size::new(1, 1)).expect("a target");
        match target.backend() {
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
}

/// What wakes the scenario's thread while it waits: the session's driver
/// work, and the completions it waits for.
#[derive(Default)]
struct Signal {
    woken: Mutex<bool>,
    changed: Condvar,
}

impl Signal {
    fn wake(&self) {
        *self.woken.lock() = true;
        self.changed.notify_all();
    }

    /// Waits until a wake since the last wait, or for `limit`.
    fn wait(&self, limit: Duration) {
        let mut woken = self.woken.lock();
        if !*woken {
            self.changed.wait_for(&mut woken, limit);
        }
        *woken = false;
    }
}

/// How a frame went.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameStatus {
    /// Whether everything the frame shows had loaded.
    pub(crate) mode: RenderMode,
    /// Whether the map changes after the frame without new commands: a
    /// transition runs, or the symbols wait for a placement.
    pub(crate) needs_repaint: bool,
}

/// A map on the runtime, over the fixtures or another file source, which a
/// render session draws on the calling thread ([`Driver::CallerThread`]).
///
/// Nothing runs on its own: the runtime's thread runs the map's commands
/// and handles what its renderer and workers report, but the workers'
/// tasks, the session's frames and its handling of the workers' results
/// run only on the scenario's thread, when it draws a frame
/// ([`draw_frame`](Self::draw_frame)) or waits for the map's background
/// work ([`wait_for_workers`](Self::wait_for_workers)), or does both until
/// the map is loaded ([`load`](Self::load), [`settle`](Self::settle),
/// [`render_still`](Self::render_still)). A frame drawn after a wait has
/// everything the frames before it asked for, so a map loads in the same
/// steps on every run. Each continuous frame is one tick of a frame clock
/// that only frames advance, which places and fades symbols and runs
/// transitions as a 60 Hz display would, however long a frame took.
pub(crate) struct HeadlessMap {
    // The session before the map, and the map before the runtime.
    session: RenderSession,
    driver: DriverWork,
    map: Map,
    runtime: Runtime,
    frontend: Frontend,
    /// Whether the session has driver work for this thread.
    driver_woken: Arc<AtomicBool>,
    signal: Arc<Signal>,
    /// The first resource that the map failed to load, which its event
    /// reported.
    failure: Arc<Mutex<Option<String>>>,
    frames: u64,
    incomplete: u64,
    /// The time of the next frame. It starts a day ahead of the system's
    /// clock, so the map never takes it for a stale frame time and always
    /// samples its animations at it.
    clock: Instant,
    /// The token of the next frame demand.
    next_token: u64,
    /// [`background_work`](Self::background_work) when the pool was last
    /// emptied.
    drained: u64,
}

impl HeadlessMap {
    /// A map in `mode`, the frontend's size, whose resources come from
    /// `file_source`, with a render session on the frontend's device. Make
    /// `file_source` run on [`Frontend::pool`].
    pub(crate) fn new(frontend: Frontend, mode: MapMode, file_source: Arc<dyn FileSource>) -> HeadlessMap {
        let runtime = mln_runtime::bench::runtime(mln_builtins::registry(), frontend.pool.clone(), file_source)
            .expect("a runtime");
        let failure = Arc::new(Mutex::new(None));
        let failed = Arc::clone(&failure);
        drop(runtime.set_event_handler(move |events| {
            for event in events {
                if let RuntimeEventKind::Map(MapEvent::LoadingFailed(error)) = &event.kind {
                    failed.lock().get_or_insert_with(|| error.to_string());
                }
            }
        }));
        let size = frontend.size;
        let options = MapOptions {
            map: mln_engine::map::MapOptions { map_mode: mode, size, pixel_ratio: 1.0, ..Default::default() },
            event_mask: EventMask::NONE.with(EventType::MapLoadingFailed),
            // The fonts of the machine would make the work differ between
            // machines: the snapshot's glyphs have every label's.
            system_fonts: false,
            local_ideographs: false,
            ..MapOptions::default()
        };
        let (map, created) = runtime.create_map(options).expect("a map");
        created.wait().expect("the map");
        let signal = Arc::clone(&frontend.signal);
        let driver_woken = Arc::new(AtomicBool::new(false));
        let (woken, wake) = (Arc::clone(&driver_woken), Arc::clone(&signal));
        let attach = AttachOptions {
            driver: Driver::CallerThread,
            driver_work_wake: Some(Arc::new(move || {
                woken.store(true, Ordering::SeqCst);
                wake.wake();
            })),
            ..AttachOptions::default()
        };
        let target = RenderTarget::Offscreen { size, device: Some(frontend.device()) };
        let (session, attached) = map.attach(target, attach).expect("a render session");
        let driver = session.take_driver_work().expect("the session's driver work");
        let clock = Instant::now() + Duration::from_hours(24);
        let map = HeadlessMap {
            session,
            driver,
            map,
            runtime,
            frontend,
            driver_woken,
            signal,
            failure,
            frames: 0,
            incomplete: 0,
            clock,
            next_token: 1,
            drained: 0,
        };
        map.wait(attached, false).expect("the session attaches");
        map
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
        map.load_style_json(style);
        map
    }

    /// Services the session's driver work, if it has any. Returns whether
    /// it had.
    fn service(&self) -> bool {
        if !self.driver_woken.swap(false, Ordering::SeqCst) {
            return false;
        }
        self.driver.service().expect("the session's driver work");
        true
    }

    /// The result of `completion`, running the session's driver work
    /// meanwhile, which a session's operation waits for, and with
    /// `run_pool` the map's background work, which some of the map's
    /// commands wait for.
    fn wait<T: Send + 'static>(&self, completion: Completion<T>, run_pool: bool) -> Result<T, mln_runtime::Error> {
        let slot = Arc::new(Mutex::new(None));
        let (result, wake) = (Arc::clone(&slot), Arc::clone(&self.signal));
        completion.on_complete(move |outcome| {
            *result.lock() = Some(outcome);
            wake.wake();
        });
        let start = Instant::now();
        loop {
            if let Some(outcome) = slot.lock().take() {
                return outcome;
            }
            self.service();
            if run_pool {
                self.frontend.pool.run_all();
            }
            if slot.lock().is_none() {
                assert!(start.elapsed() < WAIT_LIMIT, "the runtime didn't answer within {WAIT_LIMIT:?}");
                self.signal.wait(Duration::from_millis(50));
            }
        }
    }

    /// Waits until the runtime's thread has run what was sent to it before:
    /// the map's commands, and what its renderer and workers reported.
    fn round_trip(&self) {
        let completion = self.map.camera().expect("a camera query");
        self.wait(completion, false).expect("the camera");
    }

    /// Sends a command to the map and waits for it, as an application that
    /// waits for each change does. Panics, failing the scenario, when the
    /// command fails.
    fn command<T: Send + 'static>(&self, what: &str, completion: Result<Completion<T>, mln_runtime::Error>) -> T {
        let completion = completion.unwrap_or_else(|error| panic!("{what}: {error}"));
        self.wait(completion, false).unwrap_or_else(|error| panic!("{what}: {error}"))
    }

    pub(crate) fn jump(&mut self, camera: &CameraOptions) {
        let update = CameraUpdate { mode: CameraUpdateMode::Jump, camera: *camera, ..CameraUpdate::default() };
        self.command("a camera", self.map.update_camera(update));
    }

    /// Loads the style from JSON text. The style's resources load while the
    /// scenario draws and waits.
    pub(crate) fn load_style_json(&self, style: &str) {
        drop(self.map.set_style_json(style).expect("a style"));
    }

    /// Loads the style from `url`, which loads while the scenario draws and
    /// waits.
    pub(crate) fn load_style_url(&self, url: &str) {
        drop(self.map.set_style_url(url).expect("a style URL"));
    }

    /// Adds a layer to the style from its JSON.
    pub(crate) fn add_layer_json(&self, json: &str) {
        self.command("a layer", self.map.add_style_layer_json(json, None));
    }

    pub(crate) fn remove_layer(&self, id: &str) {
        self.command("a layer", self.map.remove_style_layer(id));
    }

    /// Sets a layer's property to a JSON value.
    pub(crate) fn set_layer_property(&self, layer: &str, name: &str, json: &str) {
        self.command("a layer property", self.map.set_style_layer_property(layer, name, json));
    }

    /// Sets the state of the feature `feature_id` of the source `source_id`.
    pub(crate) fn set_feature_state(&self, source_id: &str, feature_id: &str, state: FeatureState) {
        let selector = FeatureStateSelector::feature(source_id, Identifier::String(feature_id.to_owned()));
        self.command("a feature state", self.map.set_feature_state(&selector, state));
    }

    pub(crate) fn add_image(&self, image: StyleImage) {
        self.command("an image", self.map.set_style_image(image));
    }

    /// The bounds of the view at `camera`.
    pub(crate) fn lat_lng_bounds_for_camera(&self, camera: &CameraOptions) -> LatLngBounds {
        self.command("the view's bounds", self.map.lat_lng_bounds_for_camera(*camera))
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
            // The map handles what the workers sent, which can start more
            // work, such as the requests of a style that loaded, and the
            // session handles what came back to the renderer.
            self.round_trip();
            let serviced = self.service();
            if !serviced && self.background_work() == self.drained {
                self.check_failures();
                return any;
            }
            any = true;
        }
    }

    /// Draws a frame of a continuous map at the next tick of its frame
    /// clock, and waits for the GPU to finish it, as a host does before
    /// presenting a frame. Runs what is ready on the runtime's thread
    /// first, but doesn't wait for the workers. Returns how the frame went.
    pub(crate) fn draw_frame(&mut self) -> FrameStatus {
        mln_runtime::bench::hold_frame_clock(&self.map, self.clock);
        self.clock += FRAME_INTERVAL;
        // An update at this frame's time, whether or not the map had one.
        self.command("a repaint", self.map.request_repaint());
        let token = self.next_token;
        self.next_token += 1;
        self.session.demand_frame(FrameDemand::new(DemandMode::Force, token)).expect("a frame demand");
        let start = Instant::now();
        let result = loop {
            if !self.service() {
                assert!(start.elapsed() < WAIT_LIMIT, "the frame didn't render within {WAIT_LIMIT:?}");
                self.signal.wait(Duration::from_millis(50));
            }
            let results = self.session.drain_frame_results().expect("the frame results");
            if let Some(result) = results.into_iter().find(|result| result.token == Some(token)) {
                break result;
            }
        };
        assert_eq!(result.disposition, FrameDisposition::Rendered, "the frame renders");
        self.wait_for_gpu();
        self.frames += 1;
        if result.mode != RenderMode::Full {
            self.incomplete += 1;
        }
        FrameStatus { mode: result.mode, needs_repaint: result.needs_repaint }
    }

    /// Waits for the GPU to finish the frame drawn last: the native backends
    /// submit it without waiting, and WebGPU leaves it to the device.
    fn wait_for_gpu(&self) {
        let Device::WebGpu(device) = &self.frontend.device else {
            let finished = mln_runtime::bench::finish_gpu_work(&self.session).expect("a GPU wait");
            self.wait(finished, false).expect("the GPU finishes the frame");
            return;
        };
        device.device().poll(wgpu::PollType::wait_indefinitely()).expect("the GPU finishes the frame");
        if let Some(ran) = mln_webgpu::bench::take_pipeline_statistics() {
            let total = self.frontend.pipeline.get().unwrap_or_default();
            self.frontend.pipeline.set(Some(PipelineStatistics {
                vertex_shader_invocations: total.vertex_shader_invocations + ran.vertex_shader_invocations,
                primitives: total.primitives + ran.primitives,
                fragment_shader_invocations: total.fragment_shader_invocations + ran.fragment_shader_invocations,
            }));
        }
    }

    /// Draws frames, waiting for the workers before each, until one shows
    /// the view complete. Returns how many it drew.
    pub(crate) fn load(&mut self) -> u64 {
        let start = self.frames();
        loop {
            self.wait_for_workers();
            if self.draw_frame().mode == RenderMode::Full {
                return self.frames() - start;
            }
            assert!(self.frames() - start < FRAME_LIMIT, "the view isn't complete after {FRAME_LIMIT} frames");
        }
    }

    /// When the map changes next on its own, as when a symbol placement is
    /// due.
    fn repaint_at(&self) -> Option<Instant> {
        self.command("the next repaint", self.map.with_core(|map| map.repaint_at()))
    }

    /// Draws frames, waiting for the workers before each, until one shows
    /// the view complete with nothing left to animate, such as symbols
    /// fading in. When the map changes next on its own, as when a symbol
    /// placement is due, the clock skips to the frame then, as a host's
    /// display would draw nothing until it. Returns how many it drew.
    pub(crate) fn settle(&mut self) -> u64 {
        let start = self.frames();
        loop {
            self.wait_for_workers();
            let status = self.draw_frame();
            if status.mode == RenderMode::Full {
                if let Some(at) = self.repaint_at() {
                    while self.clock < at {
                        self.clock += FRAME_INTERVAL;
                    }
                    continue;
                }
                if !status.needs_repaint {
                    return self.frames() - start;
                }
            }
            assert!(self.frames() - start < FRAME_LIMIT, "the view isn't settled after {FRAME_LIMIT} frames");
        }
    }

    /// Renders a still image of a static or tile map, waiting for the
    /// workers between the renderer's updates, and reads it back from the
    /// GPU.
    pub(crate) fn render_still(&mut self) -> mln_style::bitmap::PremultipliedImage {
        let still = self.map.request_still_image().expect("a still image starts");
        let slot = Arc::new(Mutex::new(None));
        let result = Arc::clone(&slot);
        still.on_complete(move |outcome| *result.lock() = Some(outcome));
        let start = Instant::now();
        loop {
            if let Some(outcome) = slot.lock().take() {
                outcome.expect("a still image");
                return self.image();
            }
            let drew = self.service();
            let worked = self.wait_for_workers();
            if !drew && !worked {
                assert!(start.elapsed() < WAIT_LIMIT, "a still image that waits for nothing");
                self.signal.wait(Duration::from_millis(50));
            }
        }
    }

    /// Adds a GeoJSON source `id`, with the default options, holding the
    /// GeoJSON text `json`, as [`set_geojson`](Self::set_geojson) does.
    pub(crate) fn add_geojson_source(&mut self, id: &str, json: &str) {
        self.command("a new source", self.map.add_geojson_source(id, GeoJsonSourceOptions::default()));
        self.set_geojson(id, json);
    }

    /// Replaces the data of the GeoJSON source `id` with the GeoJSON text
    /// `json`, as an application's update through the runtime does: the
    /// runtime parses it, indexes it for tiling and gives it to the source.
    /// The source's tiles are cut on the map's pool.
    pub(crate) fn set_geojson(&mut self, id: &str, json: &str) {
        let set = self.map.set_geojson_source_data(id, json).expect("GeoJSON");
        self.wait(set, true).expect("GeoJSON that tiles");
    }

    /// The rendering stats of the session's renderer: what it has made and
    /// drawn so far, and holds.
    fn rendering_stats(&self) -> RenderingStats {
        let stats = mln_runtime::bench::rendering_stats(&self.session).expect("the rendering stats");
        self.wait(stats, false).expect("the rendering stats")
    }

    /// The vertex and index buffers the GPU held after the last frame.
    pub(crate) fn gpu_buffer_bytes(&self) -> u64 {
        let stats = self.rendering_stats();
        (i64::from(stats.mem_vertex_buffers) + i64::from(stats.mem_index_buffers)) as u64
    }

    /// The draw calls the renderer has made so far, those into terrain's
    /// drapes included.
    pub(crate) fn draw_calls(&self) -> u64 {
        self.rendering_stats().total_draw_calls as u64
    }

    /// The textures and buffers the renderer has made on the GPU so far.
    pub(crate) fn gpu_objects_created(&self) -> u64 {
        let stats = self.rendering_stats();
        stats.num_created_textures as u64 + stats.total_buffer_objs as u64
    }

    /// The last frame, read back from the GPU.
    pub(crate) fn image(&mut self) -> mln_style::bitmap::PremultipliedImage {
        let read = self.session.read_texture().expect("a readback");
        self.wait(read, false).expect("the frame's pixels")
    }

    /// Writes the last frame as a PNG to `path`.
    pub(crate) fn save_image(&mut self, path: &Path) {
        save_png(&self.image(), path);
    }

    /// Fails the scenario if the map failed to load something.
    fn check_failures(&self) {
        if let Some(failure) = &*self.failure.lock() {
            panic!("the map failed to load: {failure}");
        }
    }

    /// Whether the map's style has terrain.
    fn has_terrain(&self) -> bool {
        self.command("the terrain", self.map.style_root_property("terrain")).is_some()
    }
}

impl Drop for HeadlessMap {
    /// Detaches the session, letting go of its graphics objects on this
    /// thread, and releases the map and the runtime, whose threads end
    /// before the process does.
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        if let Ok(detached) = self.session.detach() {
            let _ = self.wait(detached, false);
        }
        while !self.driver.is_finished() {
            if self.driver.service().is_err() {
                break;
            }
        }
        if let Ok(released) = self.map.release() {
            let _ = released.wait();
        }
        let _ = self.runtime.clear_event_handler();
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
        let terrain = map.rendering_stats().terrain;
        self.drawn += terrain.drapes_drawn as u64;
        self.most = self.most.max(terrain.drapes as u64);
        self.most_bytes = self.most_bytes.max(terrain.mem_drapes as u64);
    }

    /// Declares in `bench`, where `map`'s style has terrain, the drapes the
    /// frames drew, `drapes_drawn`; the most drapes held after any of them,
    /// `drapes`; and the most bytes their textures held, `gpu_drapes`.
    pub(crate) fn declare(&self, bench: &mut Bench, map: &HeadlessMap) {
        if !map.has_terrain() {
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
