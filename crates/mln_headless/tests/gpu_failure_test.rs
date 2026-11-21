// Frames that fail on the GPU, through a GPU that fails a kind of pass, or
// the draws in one, as a device out of memory would: a still image ends with
// the error, on a map with terrain and on one without, and the next one draws
// once the device works; a continuous map reports the failure once, draws
// again only now and then, and shows the map once the device works.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_gfx::RendererBackend as _;
use mln_gfx::frame::{FrameTarget as _, Handoff};
use mln_gfx::gpu::{
    BindingsDesc, BindingsId, BufferDesc, BufferId, Caps, Draw, GeometryDesc, GeometryId, Gpu, GpuError, PassDesc,
    PipelineDesc, PipelineId, ReadbackId, ResourceId, SamplerId, ShaderId, ShaderKey, TextureDesc, TextureId,
    TextureRegion,
};
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_map::{Map, MapEvent, MapMode, MapOptions};
use mln_renderer::{Renderer, RendererEvent};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::error::{Error, ErrorKind};
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
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
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
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
                }},
                {{
                    "id": "line",
                    "type": "line",
                    "source": "shapes",
                    "filter": ["==", "$type", "LineString"],
                    "paint": {{"line-color": "blue", "line-width": 6}}
                }}
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
        self.gpu.create_shader(key)
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
            Fail::Passes(_) => {}
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
/// map asks to be ([`Map::repaint_at`]), and hands off only the frames that
/// didn't fail. It stands in for a [`mln_headless::HeadlessFrontend`],
/// whose target keeps its GPU to itself.
struct FailingMap {
    map: Map,
    backend: mln_webgpu::RendererBackend,
    target: OffscreenTarget<mln_webgpu::WebGpu>,
    renderer: Renderer,
    /// The generation of the last update drawn.
    drawn: u64,
    fail: Fail,
    failing: usize,
    /// The frames the target took.
    handed_off: usize,
    _pool: Pool,
}

impl FailingMap {
    fn new(mode: MapMode, terrain: bool, fail: Fail) -> Self {
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
        map.load_style_json(&style(terrain));
        let camera = map.style().get_default_camera();
        map.jump_to(&camera);
        let target = OffscreenTarget::new(size);
        FailingMap { map, backend, target, renderer, drawn: 0, fail, failing: 0, handed_off: 0, _pool: pool }
    }

    /// Takes in the renderer's reports, repaints the map if its time came,
    /// then draws the map's latest update unless it drew it already, and
    /// returns whether it did. A still that hasn't loaded draws no frame. A
    /// frame that fails says so before the target would take it.
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
            assert_eq!(failing, drawn.error().is_some(), "{:?} failed: {:?}", self.fail, drawn.error());
            let handoff = if drawn.error().is_none() {
                self.handed_off += 1;
                self.target.end_frame(gpu)
            } else {
                self.target.discard_frame();
                Handoff::Dropped
            };
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

    /// The last frame's pixels.
    fn image(&mut self) -> Result<Vec<u8>, Error> {
        Ok(self.target.read_image(self.backend.gpu())?.data)
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
    let mut failures = vec![Fail::Passes("main buffer"), Fail::Passes("render target"), Fail::DrawsIn("main buffer")];
    if terrain {
        failures.extend([Fail::Passes("drape"), Fail::DrawsIn("drape")]);
    }
    failures
}

/// A still whose one frame fails, in any of its passes or draws, on a map
/// with terrain and on one without, ends with the GPU's error rather than a
/// wrong image, and the target takes nothing of it. A still asked for right
/// after draws what a new map's does.
#[test]
fn a_still_whose_frame_fails_ends_with_the_error() {
    for terrain in [false, true] {
        let new = FailingMap::new(MapMode::Static, terrain, Fail::DrawsIn("none")).still();
        let new = new.expect("a new map's still never ended").expect("a new map's still");
        for fail in failures(terrain) {
            let mut map = FailingMap::new(MapMode::Static, terrain, fail);
            map.failing = 1;
            assert_failed(map.still(), fail);
            assert_eq!((0, 1, 0), (map.failing, map.frames(), map.handed_off), "{fail:?}");
            assert_eq!(ErrorKind::InvalidState, map.image().unwrap_err().kind, "the failed frame is readable");
            let image = map.still().expect("the next still never ended").expect("the next still");
            assert_eq!((2, 1), (map.frames(), map.handed_off), "{fail:?}");
            assert!(new == image, "the next still after {fail:?} draws other pixels than a new map's");
        }
    }
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

/// A continuous map whose frames fail for a while, from its first, reports
/// the first failure only, isn't idle, and draws again only now and then,
/// not frame after frame; the target takes none of those frames. Once the
/// device works, it draws what a map that never failed draws, and goes idle.
#[test]
fn a_continuous_map_whose_frames_fail_for_a_while_reports_once_and_recovers() {
    let idle = |events: &[MapEvent]| events.iter().any(|event| matches!(event, MapEvent::Idle));
    let errors = |events: &[MapEvent]| events.iter().filter(|event| matches!(event, MapEvent::RenderError(_))).count();
    for (terrain, fail) in [(false, Fail::Passes("main buffer")), (true, Fail::DrawsIn("drape"))] {
        let mut new = FailingMap::new(MapMode::Continuous, terrain, fail);
        assert!(new.run(Duration::from_secs(10), idle).1, "a new map never went idle");
        let expected = new.image().expect("a new map's pixels");

        let mut map = FailingMap::new(MapMode::Continuous, terrain, fail);
        map.failing = usize::MAX;
        // The map loads what it shows meanwhile.
        let (mut events, _) = map.run(Duration::from_millis(500), |_| false);
        let frames = map.frames();
        let (later, _) = map.run(Duration::from_millis(1500), |_| false);
        events.extend(later);
        assert!(!idle(&events), "{fail:?}: the map went idle while its frames failed");
        assert_eq!(1, errors(&events), "{fail:?}: {events:?}");
        // At most a second apart by then.
        let failed = map.frames() - frames;
        assert!((1..=4).contains(&failed), "{fail:?}: {failed} frames in 1.5 s");
        assert_eq!(0, map.handed_off, "{fail:?}: a failed frame was handed off");

        map.failing = 0;
        let (events, recovered) = map.run(Duration::from_secs(10), idle);
        assert!(recovered, "{fail:?}: the map never went idle once the device worked");
        assert_eq!(0, errors(&events), "{fail:?}: an error once the device works");
        assert!(expected == map.image().expect("the map's pixels"), "{fail:?}: the map draws other pixels");
    }
}
