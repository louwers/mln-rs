// Frames that fail on the GPU, through a GPU that fails a kind of pass, the
// draws in one, or a shader, as a device out of memory or a backend that
// rejects a shader would: a still image ends with the error, on a map with
// terrain and on one without, and the next one draws once the device works;
// a continuous map reports the failure once, shows what did draw, draws
// again only now and then, goes idle once the failure lasts, and shows the
// map once the device works. A still whose target can't make its textures
// ends with the error too.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_gfx::RendererBackend as _;
use mln_gfx::frame::FrameTarget as _;
use mln_gfx::gpu::{
    BindingsDesc, BindingsId, BufferDesc, BufferId, Caps, Draw, GeometryDesc, GeometryId, Gpu, GpuError, PassDesc,
    PipelineDesc, PipelineId, ReadbackId, ResourceId, SamplerId, ShaderId, ShaderKey, TextureDesc, TextureId,
    TextureRegion,
};
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_headless::HeadlessFrontend;
use mln_map::{Map, MapEvent, MapMode, MapOptions};
use mln_renderer::{Renderer, RendererEvent};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::error::{Error, ErrorKind};
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;

const SIDE: u32 = 256;

/// A DEM tile of sea level everywhere, in the Mapbox encoding: (1 · 65536 +
/// 134 · 256 + 160) · 0.1 − 10000 = 0 m.
fn sea_level_png() -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[1, 134, 160, 255]);
    }
    mln_image::encode_png(&image)
}

/// A white map with a red square around the center, a blue line across it
/// and the hillshade of flat ground, which prepares in a render target; with
/// a terrain over that ground if `terrain`.
fn style(terrain: bool) -> String {
    styled(terrain, true)
}

/// [`style`], with its line if `line`.
fn styled(terrain: bool, line: bool) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
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
            {terrain}
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
    /// Beginning the passes of this label: "drape" for the drapes', "main
    /// buffer" for the frame's, "render target" for a layer's render
    /// target's, such as a hillshade's.
    Passes(&'static str),
    /// The draws in the passes of this label, which begin.
    DrawsIn(&'static str),
    /// Making the shaders whose program's name starts so, as a backend that
    /// rejects them would: "Line" for the line layer's.
    Shaders(&'static str),
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
            Fail::Passes(_) | Fail::Shaders(_) => {}
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

impl FailingMap {
    fn new(mode: MapMode, terrain: bool, fail: Fail) -> Self {
        Self::with_style(mode, &style(terrain), fail)
    }

    fn with_style(mode: MapMode, style: &str, fail: Fail) -> Self {
        let device = mln_webgpu::Device::new(None).expect("a device");
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
        file_source
            .set_response(Kind::Glyphs, |_| Some(crate::support::file_response("test/fixtures/resources/glyphs.pbf")));
        let options = MapOptions { map_mode: mode, size, ..Default::default() };
        let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
        let events = map.renderer_events();
        let invalidate = events.clone();
        let mut renderer = Renderer::new(
            backend.gpu(),
            1.0,
            mln_text::local_glyph_rasterizer::LocalGlyphs::default(),
            Arc::new(move || invalidate.push(RendererEvent::Invalidate)),
            &pool,
        );
        renderer.set_event_queue(Some(events));
        map.load_style_json(style);
        let camera = map.style().get_default_camera();
        map.jump_to(&camera);
        let target = OffscreenTarget::new(size);
        FailingMap { map, backend, target, renderer, drawn: 0, fail, failing: 0, shown: None, _pool: pool }
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
        if let Some(frame) = self.renderer.update(gpu, &update.parameters) {
            let image = self.target.begin_frame(gpu).expect("the frame's image");
            let failing = self.failing > 0;
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
                return Some(ended.and_then(|()| Ok(self.target.read_image(self.backend.gpu())?.data)));
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

/// The kinds of failures of a map with or without `terrain`.
fn failures(terrain: bool) -> Vec<Fail> {
    let mut failures = vec![
        Fail::Passes("main buffer"),
        Fail::Passes("render target"),
        Fail::DrawsIn("main buffer"),
        Fail::Shaders("Line"),
    ];
    if terrain {
        failures.extend([Fail::Passes("drape"), Fail::DrawsIn("drape")]);
    }
    failures
}

/// A still whose one frame fails, in any of its passes or draws, on a map
/// with terrain and on one without, ends with the GPU's error rather than a
/// wrong image. A still asked for right after draws what a new map's does.
#[test]
fn a_still_whose_frame_fails_ends_with_the_error() {
    for terrain in [false, true] {
        let new = FailingMap::new(MapMode::Static, terrain, Fail::DrawsIn("none")).still();
        let new = new.expect("a new map's still never ended").expect("a new map's still");
        for fail in failures(terrain) {
            let mut map = FailingMap::new(MapMode::Static, terrain, fail);
            map.failing = 1;
            assert_failed(map.still(), fail);
            assert_eq!((0, 1), (map.failing, map.frames()), "{fail:?}");
            let image = map.still().expect("the next still never ended").expect("the next still");
            assert_eq!(2, map.frames(), "{fail:?}");
            assert!(new == image, "the next still after {fail:?} draws other pixels than a new map's");
        }
    }
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
    assert_eq!(ErrorKind::Native, error.kind, "{error}");
}

/// A still whose drapes fail to draw once, in its one frame, ends with the
/// GPU's error rather than an image without them, and the next still draws
/// them, as a new map's does.
#[test]
fn a_still_whose_drapes_fail_to_draw_once_ends_with_the_error() {
    let mut map = FailingMap::new(MapMode::Static, true, Fail::Passes("drape"));
    map.failing = 1;
    assert_failed(map.still(), map.fail);
    assert_eq!((0, 1), (map.failing, map.frames()));
    let image = map.still().expect("the next still never ended").expect("the next still");
    assert_eq!(2, map.frames());
    let new = FailingMap::new(MapMode::Static, true, map.fail).still().expect("a new map's still never ended").unwrap();
    assert!(new == image, "a new map's still draws other pixels");
}

/// A still whose drapes fail to draw in every frame ends with the GPU's
/// error, in one frame, each time it is asked for.
#[test]
fn a_still_whose_drapes_fail_to_draw_in_every_frame_ends_with_the_error() {
    let mut map = FailingMap::new(MapMode::Static, true, Fail::Passes("drape"));
    map.failing = usize::MAX;
    assert_failed(map.still(), map.fail);
    assert_eq!(1, map.frames());
    assert_failed(map.still(), map.fail);
    assert_eq!(2, map.frames());
}

fn idle(events: &[MapEvent]) -> bool {
    events.iter().any(|event| matches!(event, MapEvent::Idle))
}

fn errors(events: &[MapEvent]) -> usize {
    events.iter().filter(|event| matches!(event, MapEvent::RenderError(_))).count()
}

/// What a continuous map of `style` shows once it goes idle, on a GPU that
/// works.
fn idle_image(style: &str) -> Vec<u8> {
    let mut map = FailingMap::with_style(MapMode::Continuous, style, Fail::DrawsIn("none"));
    assert!(map.run(Duration::from_secs(10), idle).1, "a new map never went idle");
    map.shown.take().expect("a new map showed nothing")
}

/// A continuous map whose line's shader always fails, as on a backend that
/// rejects it, with terrain and without, shows the rest of the map, as a
/// map without the line does, and reports the failure once. Once its frames
/// have failed for a second, it goes idle and draws no more on its own;
/// whatever else draws a frame, such as a camera change, still draws.
#[test]
fn a_continuous_map_whose_draw_always_fails_shows_the_rest_and_goes_idle() {
    for terrain in [false, true] {
        let expected = idle_image(&styled(terrain, false));
        assert!(expected != idle_image(&style(terrain)), "terrain {terrain}: the line draws nothing");
        let mut map = FailingMap::new(MapMode::Continuous, terrain, Fail::Shaders("Line"));
        map.failing = usize::MAX;
        let start = Instant::now();
        let (events, went_idle) = map.run(Duration::from_secs(10), idle);
        assert!(went_idle, "terrain {terrain}: the map never went idle: {events:?}");
        assert!(start.elapsed() >= Duration::from_secs(1), "terrain {terrain}: idle before the failure lasted");
        assert_eq!(1, errors(&events), "terrain {terrain}: {events:?}");
        assert!(map.shown.as_ref() == Some(&expected), "terrain {terrain}: the map shows other pixels");

        let frames = map.frames();
        let (events, _) = map.run(Duration::from_millis(1500), |_| false);
        assert!(frames == map.frames() && events.is_empty(), "terrain {terrain}: the idle map drew: {events:?}");
        map.map.jump_to(&mln_map::CameraOptions { zoom: Some(2.5), ..Default::default() });
        let (events, went_idle) = map.run(Duration::from_secs(10), idle);
        assert!(went_idle && map.frames() > frames, "terrain {terrain}: {events:?}");
        assert_eq!(0, errors(&events), "terrain {terrain}: the same failure reported again");
    }
}

/// A continuous map whose frames fail for half a second, from its first,
/// reports the first failure only, isn't idle, and draws again only now and
/// then, not frame after frame. Once the device works, it draws again on its
/// own, shows what a map that never failed shows, and goes idle.
#[test]
fn a_continuous_map_whose_frames_fail_for_a_while_reports_once_and_recovers() {
    for (terrain, fail) in
        [(false, Fail::Passes("main buffer")), (true, Fail::DrawsIn("drape")), (false, Fail::Shaders("Line"))]
    {
        let expected = idle_image(&style(terrain));
        let mut map = FailingMap::new(MapMode::Continuous, terrain, fail);
        map.failing = usize::MAX;
        // The map loads what it shows meanwhile.
        let (mut events, _) = map.run(Duration::from_millis(200), |_| false);
        let frames = map.frames();
        let (later, _) = map.run(Duration::from_millis(300), |_| false);
        events.extend(later);
        assert!(!idle(&events), "{fail:?}: the map went idle while its frames failed");
        assert_eq!(1, errors(&events), "{fail:?}: {events:?}");
        let failed = map.frames() - frames;
        assert!(failed <= 4, "{fail:?}: {failed} frames in 300 ms");

        map.failing = 0;
        let (events, recovered) = map.run(Duration::from_secs(10), idle);
        assert!(recovered, "{fail:?}: the map never went idle once the device worked");
        assert_eq!(0, errors(&events), "{fail:?}: an error once the device works");
        assert!(map.shown.as_ref() == Some(&expected), "{fail:?}: the map shows other pixels");
    }
}

/// A map over flat terrain, seen pitched, of a red label at the center, the
/// only layer, with a square around the center, which drapes, for a layer
/// added later.
const LABEL_STYLE: &str = r#"{
    "version": 8,
    "center": [0, 0],
    "zoom": 4,
    "pitch": 40,
    "glyphs": "glyphs/{fontstack}/{range}.pbf",
    "terrain": {"source": "dem"},
    "sources": {
        "dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 10},
        "square": {"type": "geojson", "data": {"type": "Polygon", "coordinates": [[[-5, -5], [5, -5], [5, 5], [-5, 5], [-5, -5]]]}},
        "label": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}
    },
    "layers": [
        {"id": "label", "type": "symbol", "source": "label",
         "layout": {"text-field": "HH", "text-font": ["Open Sans Regular"], "text-size": 24},
         "paint": {"text-color": "red"}}
    ]
}"#;

/// How many of the RGBA `pixels` are `kind`.
fn count(pixels: &[u8], kind: fn([u8; 4]) -> bool) -> usize {
    pixels.as_chunks::<4>().0.iter().filter(|&&pixel| kind(pixel)).count()
}

/// A continuous map over terrain whose shader of the ground's depth always
/// fails, as on a backend that rejects it, reports the failure once, and
/// shows its label, in front of the ground, which then hides nothing. A
/// layer that drapes, added once its frames have failed without one, draws
/// on the surface.
#[test]
fn a_continuous_map_whose_ground_depth_shader_fails_hides_nothing_and_draws_the_surface() {
    let mut map = FailingMap::with_style(MapMode::Continuous, LABEL_STYLE, Fail::Shaders("TerrainDepth"));
    map.failing = usize::MAX;
    let (mut events, went_idle) = map.run(Duration::from_secs(10), idle);
    assert!(went_idle, "the map never went idle");
    let red = |[r, g, b, _]: [u8; 4]| r > 200 && g < 60 && b < 60;
    let shown = map.shown.take().expect("the map showed nothing");
    assert!(count(&shown, red) >= 40, "{} opaque red pixels of the label", count(&shown, red));

    let square = r#"{"id": "square", "type": "fill", "source": "square", "paint": {"fill-color": "lime"}}"#;
    map.map.edit_style(|style| style.add_layer_json(square, Some("label"))).unwrap();
    let (later, went_idle) = map.run(Duration::from_secs(10), idle);
    events.extend(later);
    assert!(went_idle, "the map never went idle with the square");
    let shown = map.shown.take().expect("the map showed nothing");
    let lime = |[r, g, b, _]: [u8; 4]| r < 60 && g > 200 && b < 60;
    assert!(count(&shown, lime) >= 1000, "{} lime pixels of the square", count(&shown, lime));
    assert!(count(&shown, red) >= 40, "{} opaque red pixels of the label over the square", count(&shown, red));
    let reported: Vec<_> = events.iter().filter(|event| matches!(event, MapEvent::RenderError(_))).collect();
    assert_eq!(1, reported.len(), "{reported:?}");
}
