// Frames that fail on the GPU, through a GPU that fails a kind of pass, the
// draws in one, or a shader, as a device out of memory or a backend that
// rejects a shader would: a still image ends with the error, and the next
// one draws once the device works;
// a continuous map reports the failure once, shows what did draw, draws
// again only now and then, goes idle once the failure lasts, and shows the
// map once the device works. A still whose target can't make its textures
// ends with the error too.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_engine::map::{Map, MapEvent, MapMode, MapOptions};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{Kind, Resource, Response};
use mln_engine::{Renderer, RendererEvent};
use mln_gfx::RendererBackend as _;
use mln_gfx::frame::FrameTarget as _;
use mln_gfx::gpu::{
    BindingsDesc, BindingsId, BufferDesc, BufferId, Caps, Draw, GeometryDesc, GeometryId, Gpu, GpuError, PassDesc,
    PipelineDesc, PipelineId, ReadbackId, ResourceId, SamplerId, ShaderId, ShaderKey, TextureDesc, TextureId,
    TextureRegion,
};
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_headless::HeadlessFrontend;
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::{Error, ErrorKind};
use mln_style::size::Size;
use mln_util::pool::Pool;
use mln_util::queue::Queue;

const SIDE: u32 = 256;

/// A DEM tile of sea level everywhere, in the Mapbox encoding: (1 · 65536 +
/// 134 · 256 + 160) · 0.1 − 10000 = 0 m.
fn sea_level_png() -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[1, 134, 160, 255]);
    }
    mln_engine::image::encode_png(&image)
}

/// A white map with a red square around the center, a blue line across it
/// and the hillshade of flat ground, which prepares in a render target.
fn style() -> String {
    styled(true)
}

/// [`style`], with its line if `line`.
fn styled(line: bool) -> String {
    let line = if line {
        r#",{
            "id": "line",
            "type": "line",
            "source": "shapes",
            "filter": ["==", "$type", "LineString"],
            "paint": {"line-color": "blue", "line-width": 6}
        }"#
    } else {
        ""
    };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 2,
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                "shapes": {{
                    "type": "geojson",
                    "data": {{
                        "type": "FeatureCollection",
                        "features": [
                            {{
                                "type": "Feature",
                                "properties": {{}},
                                "geometry": {{
                                    "type": "Polygon",
                                    "coordinates": [[[-5, -5], [5, -5], [5, 5], [-5, 5], [-5, -5]]]
                                }}
                            }},
                            {{
                                "type": "Feature",
                                "properties": {{}},
                                "geometry": {{"type": "LineString", "coordinates": [[-60, 10], [60, 10]]}}
                            }}
                        ]
                    }}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "hillshade", "type": "hillshade", "source": "dem"}},
                {{
                    "id": "fill",
                    "type": "fill",
                    "source": "shapes",
                    "filter": ["==", "$type", "Polygon"],
                    "paint": {{"fill-color": "red"}}
                }}
                {line}
            ]
        }}"#
    )
}

/// What a [`Failing`] GPU fails.
#[derive(Clone, Copy, Debug)]
enum Fail {
    /// Beginning the passes of this label: "main buffer" for the frame's,
    /// "render target" for a layer's render target's, such as a
    /// hillshade's.
    Passes(&'static str),
    /// The draws in the passes of this label, which begin.
    DrawsIn(&'static str),
    /// Making the shaders whose program's name starts so, as a backend that
    /// rejects them would: "Line" for the line layer's.
    Shaders(&'static str),
    /// Submitting the frame.
    Submits,
}

/// A GPU that fails what `fail` says, and does the rest on `gpu`.
struct Failing<'a> {
    gpu: &'a mut dyn Gpu,
    fail: Fail,
    /// Whether the pass open is one whose draws fail.
    failing_pass: bool,
}

fn failed<T>() -> Result<T, GpuError> {
    Err(GpuError::Invalid("failed on purpose".to_owned()))
}

impl Gpu for Failing<'_> {
    fn caps(&self) -> &Caps {
        self.gpu.caps()
    }
    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        self.gpu.create_buffer(desc)
    }
    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        self.gpu.write_buffer(buffer, offset, data)
    }
    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError> {
        self.gpu.create_texture(desc)
    }
    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError> {
        self.gpu.write_texture(texture, region, data)
    }
    fn generate_mipmaps(&mut self, texture: TextureId) -> Result<(), GpuError> {
        self.gpu.generate_mipmaps(texture)
    }
    fn create_sampler(&mut self, state: &mln_gfx::texture2d::SamplerState) -> Result<SamplerId, GpuError> {
        self.gpu.create_sampler(state)
    }
    fn create_shader(&mut self, key: &ShaderKey) -> Result<ShaderId, GpuError> {
        match self.fail {
            Fail::Shaders(name) if key.program().name.starts_with(name) => failed(),
            _ => self.gpu.create_shader(key),
        }
    }
    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<PipelineId, GpuError> {
        self.gpu.create_pipeline(desc)
    }
    fn create_bindings(&mut self, desc: &BindingsDesc<'_>) -> Result<BindingsId, GpuError> {
        self.gpu.create_bindings(desc)
    }
    fn create_geometry(&mut self, desc: &GeometryDesc<'_>) -> Result<GeometryId, GpuError> {
        self.gpu.create_geometry(desc)
    }
    fn destroy(&mut self, resource: ResourceId) -> Result<(), GpuError> {
        self.gpu.destroy(resource)
    }
    fn begin_pass(&mut self, desc: &PassDesc<'_>) -> Result<(), GpuError> {
        match self.fail {
            Fail::Passes(label) if desc.label == label => return failed(),
            Fail::DrawsIn(label) => self.failing_pass = desc.label == label,
            Fail::Passes(_) | Fail::Shaders(_) | Fail::Submits => {}
        }
        self.gpu.begin_pass(desc)
    }
    fn attaches_with_any_color(&self, depth_stencil: TextureId) -> bool {
        self.gpu.attaches_with_any_color(depth_stencil)
    }
    fn draw(&mut self, draw: &Draw) -> Result<(), GpuError> {
        if self.failing_pass {
            return failed();
        }
        self.gpu.draw(draw)
    }
    fn lend(&mut self, f: &mut dyn FnMut(&mut dyn std::any::Any)) -> Result<(), GpuError> {
        self.gpu.lend(f)
    }
    fn end_pass(&mut self) -> Result<(), GpuError> {
        self.failing_pass = false;
        self.gpu.end_pass()
    }
    fn flush(&mut self, wait: bool) -> Result<(), GpuError> {
        if matches!(self.fail, Fail::Submits) {
            return failed();
        }
        self.gpu.flush(wait)
    }
    fn read_texture(&mut self, texture: TextureId, region: TextureRegion) -> Result<ReadbackId, GpuError> {
        self.gpu.read_texture(texture, region)
    }
    fn take_readback(&mut self, readback: ReadbackId, wait: bool) -> Result<Option<Vec<u8>>, GpuError> {
        self.gpu.take_readback(readback, wait)
    }
}

/// A map of [`style`], and a renderer on WebGPU that draws each update the
/// map publishes, as a frontend does, through a GPU that fails what `fail`
/// says in the next `failing` frames it draws. It repaints the map when the
/// map asks to be ([`Map::repaint_at`]), and keeps the pixels of the last
/// frame, as a surface shows them. It stands in for a
/// [`mln_headless::HeadlessFrontend`], whose target keeps its GPU to itself.
struct FailingMap {
    map: Map,
    backend: mln_webgpu::RendererBackend,
    target: OffscreenTarget<mln_webgpu::WebGpu>,
    renderer: Renderer,
    /// The generation of the last update drawn.
    drawn: u64,
    fail: Fail,
    failing: usize,
    /// The pixels of the last frame.
    shown: Option<Vec<u8>>,
    _pool: Pool,
}

/// A WebGPU device, or `None` where no adapter is available: the test is
/// skipped there, unless `MLN_RENDER_BACKEND` asks for `webgpu`.
fn webgpu_device() -> Option<mln_webgpu::Device> {
    let requested = std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case("webgpu"));
    match mln_webgpu::Device::new(None) {
        Ok(device) => Some(device),
        Err(error) if error.kind == mln_gfx::ErrorKind::Unsupported && !requested => {
            eprintln!("skipped: {error}");
            None
        }
        Err(error) => panic!("WebGPU device: {error}"),
    }
}

impl FailingMap {
    fn new(mode: MapMode, fail: Fail) -> Option<Self> {
        Self::with_style(mode, &style(), fail)
    }

    fn with_style(mode: MapMode, style: &str, fail: Fail) -> Option<Self> {
        let device = webgpu_device()?;
        let mut backend = mln_webgpu::RendererBackend::new(&device);
        let pool = Pool::new(4);
        let size = Size::new(SIDE, SIDE);
        let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
        let dem = Arc::new(sea_level_png());
        file_source.set_response(Kind::Tile, move |_: &Resource| {
            let mut response = Response::new();
            response.data = Some(Arc::clone(&dem));
            Some(response)
        });
        let options = MapOptions { map_mode: mode, size, ..Default::default() };
        let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
        let events = map.renderer_events();
        let invalidate = events.clone();
        let mut renderer = Renderer::new(
            backend.gpu(),
            1.0,
            mln_engine::text::local_fonts::LocalGlyphs::default(),
            Arc::new(move || invalidate.push(RendererEvent::Invalidate)),
            &pool,
        );
        renderer.set_event_queue(Some(events));
        map.load_style_json(style);
        let camera = mln_engine::map::style_camera(map.style());
        map.jump_to(&camera);
        let target = OffscreenTarget::new(mln_gfx::Size::new(size.width, size.height));
        Some(FailingMap { map, backend, target, renderer, drawn: 0, fail, failing: 0, shown: None, _pool: pool })
    }

    /// Takes in the renderer's reports, repaints the map if its time came,
    /// then draws the map's latest update unless it drew it already, and
    /// returns whether it did. A still that hasn't loaded draws no frame.
    fn draw(&mut self) -> bool {
        self.map.process_events();
        if self.map.repaint_at().is_some_and(|at| Instant::now() >= at) {
            self.map.trigger_repaint();
        }
        let Some(update) = self.map.update_reader().latest().filter(|update| update.generation != self.drawn) else {
            return false;
        };
        self.drawn = update.generation;
        let gpu = self.backend.gpu();
        let failing = self.failing > 0;
        let updated = if failing {
            self.renderer.update(&mut Failing { gpu, fail: self.fail, failing_pass: false }, &update.parameters)
        } else {
            self.renderer.update(gpu, &update.parameters)
        };
        if let Some(frame) = updated {
            let image = self.target.begin_frame(gpu).expect("the frame's image");
            let drawn = if failing {
                self.failing -= 1;
                self.renderer.render(&mut Failing { gpu, fail: self.fail, failing_pass: false }, &image, frame)
            } else {
                self.renderer.render(gpu, &image, frame)
            };
            // A frame fails through a shader only once it draws with it.
            let failed = drawn.error().is_some();
            let shader = matches!(self.fail, Fail::Shaders(_));
            assert!(failed == failing || (failing && shader), "{:?} failed: {:?}", self.fail, drawn.error());
            let handoff = self.target.end_frame(gpu);
            self.shown = Some(self.target.read_image(gpu).expect("the frame's pixels").data);
            self.renderer.finish_frame(gpu, drawn, handoff);
        }
        true
    }

    /// Asks for a still and draws the map's updates until it ends, with its
    /// pixels, or gives up after a while: `None` for a still that never
    /// ends.
    fn still(&mut self) -> Option<Result<Vec<u8>, Error>> {
        self.map.process_events();
        self.map.render_still().unwrap();
        for _ in 0..2_000 {
            self.map.process_events();
            if let Some(ended) = self.map.take_still_image() {
                return Some(
                    ended.map(|()| self.target.read_image(self.backend.gpu()).expect("the frame's pixels").data),
                );
            }
            if !self.draw() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        None
    }

    /// Draws until `done` says so of the map's events, or `within` passed,
    /// and returns the events and whether `done` said so.
    fn run(&mut self, within: Duration, mut done: impl FnMut(&[MapEvent]) -> bool) -> (Vec<MapEvent>, bool) {
        let mut events = Vec::new();
        let start = Instant::now();
        while start.elapsed() < within {
            if !self.draw() {
                std::thread::sleep(Duration::from_millis(1));
            }
            events.extend(self.map.drain_events());
            if done(&events) {
                return (events, true);
            }
        }
        (events, false)
    }

    /// The frames the renderer drew.
    fn frames(&self) -> i32 {
        self.renderer.rendering_stats().num_frames
    }
}

impl Drop for FailingMap {
    fn drop(&mut self) {
        let gpu = self.backend.gpu();
        self.target.tear_down(gpu);
    }
}

/// Asserts that a still ended with the error of a GPU that failed on
/// purpose.
fn assert_failed(ended: Option<Result<Vec<u8>, Error>>, fail: Fail) {
    let error = ended.expect("the still never ended").expect_err(&format!("the still drew through {fail:?}"));
    assert_eq!(ErrorKind::Native, error.kind);
    assert!(error.message.contains("failed on purpose"), "{fail:?}: {error}");
}

/// The kinds of failures of a map.
const FAILURES: [Fail; 6] = [
    Fail::Passes("main buffer"),
    Fail::Passes("render target"),
    Fail::DrawsIn("main buffer"),
    Fail::DrawsIn("render target"),
    Fail::Shaders("Line"),
    Fail::Submits,
];

/// A still whose one frame fails, in any of its passes or draws, ends with
/// the GPU's error rather than a wrong image. A still asked for right after
/// draws what a new map's does.
#[test]
fn a_still_whose_frame_fails_ends_with_the_error() {
    let Some(mut new) = FailingMap::new(MapMode::Static, Fail::DrawsIn("none")) else {
        return;
    };
    let new = new.still().expect("a new map's still never ended").expect("a new map's still");
    for fail in FAILURES {
        let Some(mut map) = FailingMap::new(MapMode::Static, fail) else {
            return;
        };
        map.failing = 1;
        assert_failed(map.still(), fail);
        assert_eq!((0, 1), (map.failing, map.frames()), "{fail:?}");
        let image = map.still().expect("the next still never ended").expect("the next still");
        assert_eq!(2, map.frames(), "{fail:?}");
        assert!(new == image, "the next still after {fail:?} draws other pixels than a new map's");
    }
}

/// A still whose frames fail in every frame ends with the GPU's error, in
/// one frame, each time it is asked for.
#[test]
fn a_still_whose_frames_always_fail_ends_with_the_error_each_time() {
    let Some(mut map) = FailingMap::new(MapMode::Static, Fail::Passes("render target")) else {
        return;
    };
    map.failing = usize::MAX;
    assert_failed(map.still(), map.fail);
    assert_eq!(1, map.frames());
    assert_failed(map.still(), map.fail);
    assert_eq!(2, map.frames());
}

/// A still whose target can't make the textures to draw it into, here for
/// a size that no device takes, ends with the error rather than waiting.
#[test]
fn a_still_whose_target_cant_make_its_textures_ends_with_the_error() {
    let (queue, pool) = (Queue::new(), Pool::new(4));
    let size = Size::new(1 << 17, 1);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &queue, &pool).expect("a headless frontend");
    let mut map = Map::new(MapOptions { map_mode: MapMode::Static, size, ..Default::default() }, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(r#"{"version": 8, "sources": {}, "layers": []}"#);
    let error = frontend.render(&mut map).err().expect("a still of a target without textures");
    assert_eq!(ErrorKind::InvalidState, error.kind, "{error}");
}

fn idle(events: &[MapEvent]) -> bool {
    events.iter().any(|event| matches!(event, MapEvent::Idle))
}

fn errors(events: &[MapEvent]) -> usize {
    events.iter().filter(|event| matches!(event, MapEvent::RenderError(_))).count()
}

/// What a continuous map of `style` shows once it goes idle, on a GPU that
/// works.
fn idle_image(style: &str) -> Option<Vec<u8>> {
    let mut map = FailingMap::with_style(MapMode::Continuous, style, Fail::DrawsIn("none"))?;
    assert!(map.run(Duration::from_secs(10), idle).1, "a new map never went idle");
    Some(map.shown.take().expect("a new map showed nothing"))
}

/// A continuous map whose line's shader always fails, as on a backend that
/// rejects it, shows the rest of the map, as a map without the line does,
/// and reports the failure once. It goes idle, after drawing again once, and
/// draws no more on its own; whatever else draws a frame, such as a camera
/// change, still draws.
#[test]
fn a_continuous_map_whose_draw_always_fails_shows_the_rest_and_goes_idle() {
    let Some(expected) = idle_image(&styled(false)) else {
        return;
    };
    let Some(with_line) = idle_image(&style()) else {
        return;
    };
    assert!(expected != with_line, "the line draws nothing");
    let Some(mut map) = FailingMap::new(MapMode::Continuous, Fail::Shaders("Line")) else {
        return;
    };
    map.failing = usize::MAX;
    let (events, went_idle) = map.run(Duration::from_secs(10), idle);
    assert!(went_idle, "the map never went idle: {events:?}");
    assert_eq!(1, errors(&events), "{events:?}");
    assert!(map.shown.as_ref() == Some(&expected), "the map shows other pixels");

    let frames = map.frames();
    let (events, _) = map.run(Duration::from_millis(1500), |_| false);
    assert!(frames == map.frames() && events.is_empty(), "the idle map drew: {events:?}");
    map.map.jump_to(&mln_engine::camera::CameraOptions { zoom: Some(2.5), ..Default::default() });
    let (events, went_idle) = map.run(Duration::from_secs(10), idle);
    assert!(went_idle && map.frames() > frames, "{events:?}");
    assert_eq!(0, errors(&events), "the same failure reported again");
}

/// A still of a terrain on a backend that fails to make the terrain's
/// shader draws the map flat, its draped layers too, as a map without
/// terrain does, with the camera on no ground.
#[test]
fn a_still_without_the_terrains_shader_draws_flat() {
    let Some(mut flat) = FailingMap::new(MapMode::Static, Fail::DrawsIn("none")) else {
        return;
    };
    let flat = flat.still().expect("a flat map's still never ended").expect("a flat map's still");
    let style = style().replacen(r#""version": 8,"#, r#""version": 8, "terrain": {"source": "dem"},"#, 1);
    let Some(mut map) = FailingMap::with_style(MapMode::Static, &style, Fail::Shaders("TerrainShader")) else {
        return;
    };
    map.failing = usize::MAX;
    let image = map.still().expect("the still never ended").expect("the still");
    assert!(flat == image, "the still draws other pixels than a flat map's");
    assert_eq!(None, map.map.transform_state().view().terrain_min_elevation(), "the camera stands on ground");
}

/// A continuous map whose frame fails once, as for a failure that passes,
/// reports it and draws again on its own a little later, which shows what a
/// map that never failed shows, and goes idle.
#[test]
fn a_continuous_map_whose_frame_fails_once_reports_it_and_draws_again() {
    let Some(expected) = idle_image(&style()) else {
        return;
    };
    // A shader that failed would fail every frame: only a pass can fail once.
    for fail in [Fail::Passes("main buffer"), Fail::DrawsIn("main buffer")] {
        let Some(mut map) = FailingMap::new(MapMode::Continuous, fail) else {
            return;
        };
        assert!(map.run(Duration::from_secs(10), idle).1, "{fail:?}: a new map never went idle");
        let frames = map.frames();
        map.failing = 1;
        map.map.trigger_repaint();
        let (events, went_idle) = map.run(Duration::from_secs(10), idle);
        assert!(went_idle, "{fail:?}: the map never went idle: {events:?}");
        assert_eq!(1, errors(&events), "{fail:?}: {events:?}");
        assert_eq!(frames + 2, map.frames(), "{fail:?}: the failed frame and the one that drew it again");
        assert!(map.shown.as_ref() == Some(&expected), "{fail:?}: the map shows other pixels");
    }
}
