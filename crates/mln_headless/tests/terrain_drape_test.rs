// The layers that drape over terrain draw into the terrain tiles' drapes,
// which the surface shows: over flat ground, seen from above, the map looks
// as it does without terrain. A drape that fails to draw fails a still image
// with the GPU's error, and a continuous map draws it again.

use std::sync::Arc;

use mln_gfx::RendererBackend as _;
use mln_gfx::frame::FrameTarget as _;
use mln_gfx::gpu::{
    BindingsDesc, BindingsId, BufferDesc, BufferId, Caps, Draw, GeometryDesc, GeometryId, Gpu, GpuError, PassDesc,
    PipelineDesc, PipelineId, ReadbackId, ResourceId, SamplerId, ShaderId, ShaderKey, TextureDesc, TextureId,
    TextureRegion,
};
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, Map, MapEvent, MapMode, MapOptions, ViewportMode};
use mln_renderer::{Renderer, RendererEvent};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::error::{Error, ErrorKind};
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;

const SIDE: u32 = 256;

/// How many pixels a draped map may differ by from the same map drawn flat:
/// the surface filters its drapes, so pixels on the shapes' edges differ, and
/// how many depends on the GPU.
const EDGE_PIXELS: u64 = 64;

/// A DEM tile of sea level everywhere, in the Mapbox encoding: (1 · 65536 +
/// 134 · 256 + 160) · 0.1 − 10000 = 0 m.
fn sea_level_png() -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[1, 134, 160, 255]);
    }
    mln_image::encode_png(&image)
}

/// A white map with a red square around the center and a blue line across
/// it, at an integer zoom, seen from above; with a terrain over flat ground
/// if `terrain`.
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

/// A white map with a red square around the center, a green circle at the
/// center, and a blue rectangle over the circle's east half, at an integer
/// zoom, seen from above; with a terrain over flat ground if `terrain`. The
/// circle splits the draped layers into two stacks.
fn stacked_style(terrain: bool) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 2,
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                "red": {{
                    "type": "geojson",
                    "data": {{"type": "Polygon", "coordinates": [[[-8, -8], [8, -8], [8, 8], [-8, 8], [-8, -8]]]}}
                }},
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, 0]}}}},
                "blue": {{
                    "type": "geojson",
                    "data": {{"type": "Polygon", "coordinates": [[[-1, -5], [5, -5], [5, 5], [-1, 5], [-1, -5]]]}}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "red", "type": "fill", "source": "red", "paint": {{"fill-color": "red"}}}},
                {{
                    "id": "circle",
                    "type": "circle",
                    "source": "point",
                    "paint": {{"circle-color": "lime", "circle-radius": 15, "circle-blur": 0}}
                }},
                {{"id": "blue", "type": "fill", "source": "blue", "paint": {{"fill-color": "blue"}}}}
            ]
        }}"#
    )
}

/// Where the blue fill of [`fogged_style`] is: in the upper stack, in the
/// lower one, or nowhere.
#[derive(Clone, Copy, PartialEq)]
enum Blue {
    Above,
    Below,
    None,
}

/// A white map over flat terrain pitched 75°, under a magenta fog over the
/// whole ground, with a circle near the camera and an opaque blue fill over
/// the ground west of 1° W, in the stack above the circle or below it.
fn fogged_style(blue: Blue) -> String {
    let fill = r#"{"id": "blue", "type": "fill", "source": "blue", "paint": {"fill-color": "blue"}}"#;
    let circle = r#"{"id": "circle", "type": "circle", "source": "point", "paint": {"circle-color": "lime"}}"#;
    let layers = match blue {
        Blue::Above => format!("{circle}, {fill}"),
        Blue::Below => format!("{fill}, {circle}"),
        Blue::None => circle.to_owned(),
    };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 4,
            "pitch": 75,
            "terrain": {{"source": "dem"}},
            "sky": {{"fog-color": "magenta", "fog-ground-blend": 0}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, -4]}}}},
                "blue": {{
                    "type": "geojson",
                    "data": {{"type": "Polygon", "coordinates": [[[-170, -80], [-1, -80], [-1, 80], [-170, 80], [-170, -80]]]}}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {layers}
            ]
        }}"#
    )
}

/// The still image of `style` on the harness's device
/// ([`mln_headless::harness_device`]), and the bytes of the textures that
/// drew it; `None` where the machine has no such device.
fn render(style: &str, viewport_mode: ViewportMode) -> Option<(PremultipliedImage, i32)> {
    let size = Size::new(SIDE, SIDE);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let device = match mln_headless::harness_device() {
        Ok(device) => device,
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no device: {error}");
            return None;
        }
        Err(error) => panic!("a device: {error}"),
    };
    let mut frontend =
        HeadlessFrontend::with_device(device, size, 1.0, Arc::default(), &run_loop, &pool).expect("a frontend");
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    let dem = Arc::new(sea_level_png());
    file_source.set_response(Kind::Tile, move |_: &Resource| {
        let mut response = Response::new();
        response.data = Some(Arc::clone(&dem));
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, viewport_mode, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.load_style_json(style);
    let camera = map.style().get_default_camera();
    map.jump_to(&camera);
    let result = frontend.render(&mut map).expect("the still image");
    Some((result.image, result.stats.mem_textures))
}

fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * image.size.width + x) * 4) as usize;
    image.data[start..start + 4].try_into().unwrap()
}

#[test]
fn a_fill_and_a_line_drape_over_flat_terrain() {
    let (Some((draped, draped_textures)), Some((flat, flat_textures))) =
        (render(&style(true), ViewportMode::Default), render(&style(false), ViewportMode::Default))
    else {
        return;
    };
    // At least one drape of 1024² texels for the terrain's 512 px tiles.
    assert!(draped_textures - flat_textures >= 1024 * 1024 * 4, "the drapes' textures");
    let center = SIDE / 2;
    assert_eq!([255, 0, 0, 255], pixel(&draped, center, center), "the fill");
    // 10° north is 57 px north of the equator in the 2048 px of the world
    // at zoom 2.
    assert_eq!([0, 0, 255, 255], pixel(&draped, center, center - 57), "the line");
    assert_eq!([255, 255, 255, 255], pixel(&draped, 4, 4), "the background");
    let differing = pixelmatch::pixelmatch(&draped.data, &flat.data, SIDE as usize, SIDE as usize, None, 0.1285, false);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A viewport flipped upside down, which turns the surface's triangles
/// round, still shows the surface, drawn upside down.
#[test]
fn a_flipped_viewport_shows_the_terrain() {
    let (Some((flipped, _)), Some((flat, _))) =
        (render(&style(true), ViewportMode::FlippedY), render(&style(false), ViewportMode::FlippedY))
    else {
        return;
    };
    let center = SIDE / 2;
    assert_eq!([0, 0, 255, 255], pixel(&flipped, center, center + 57), "the line, south of the center");
    let differing =
        pixelmatch::pixelmatch(&flipped.data, &flat.data, SIDE as usize, SIDE as usize, None, 0.1285, false);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A circle between two fills stands between their stacks: the fill below
/// shows under it, and the fill above covers it, as without terrain.
#[test]
fn a_circle_stands_between_two_stacks_of_draped_layers() {
    let (Some((draped, _)), Some((flat, _))) =
        (render(&stacked_style(true), ViewportMode::Default), render(&stacked_style(false), ViewportMode::Default))
    else {
        return;
    };
    let center = SIDE / 2;
    assert_eq!([0, 0, 255, 255], pixel(&draped, center, center), "the blue fill over the circle's center");
    // 1° is 5.7 px in the 2048 px of the world at zoom 2: the blue fill
    // starts 6 px west of the center, and the circle reaches 15 px.
    assert_eq!([0, 255, 0, 255], pixel(&draped, center - 11, center), "the circle west of the blue fill");
    assert_eq!([255, 0, 0, 255], pixel(&draped, center - 30, center), "the red fill around the circle");
    let differing = pixelmatch::pixelmatch(&draped.data, &flat.data, SIDE as usize, SIDE as usize, None, 0.1285, false);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A GPU that fails the drapes' passes, as a device out of memory would,
/// and does the rest on `gpu`.
struct FailingDrapes<'a> {
    gpu: &'a mut dyn Gpu,
}

impl Gpu for FailingDrapes<'_> {
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
        if desc.label == "drape" {
            return Err(GpuError::Invalid("failed on purpose".to_owned()));
        }
        self.gpu.begin_pass(desc)
    }
    fn attaches_with_any_color(&self, depth_stencil: TextureId) -> bool {
        self.gpu.attaches_with_any_color(depth_stencil)
    }
    fn draw(&mut self, draw: &Draw) -> Result<(), GpuError> {
        self.gpu.draw(draw)
    }
    fn lend(&mut self, f: &mut dyn FnMut(&mut dyn std::any::Any)) -> Result<(), GpuError> {
        self.gpu.lend(f)
    }
    fn end_pass(&mut self) -> Result<(), GpuError> {
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

/// A map of [`style`] over flat terrain, and a renderer on WebGPU that
/// draws each update the map publishes, as a frontend does, through a GPU
/// that fails the drapes' passes in the next `failing` frames it draws. It
/// stands in for a [`HeadlessFrontend`], whose target keeps its GPU to
/// itself.
struct FailingMap {
    map: Map,
    backend: mln_webgpu::RendererBackend,
    target: OffscreenTarget<mln_webgpu::WebGpu>,
    renderer: Renderer,
    /// The generation of the last update drawn.
    drawn: u64,
    failing: usize,
    _pool: Pool,
}

impl FailingMap {
    fn new(mode: MapMode) -> Self {
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
        map.load_style_json(&style(true));
        let camera = map.style().get_default_camera();
        map.jump_to(&camera);
        let target = OffscreenTarget::new(size);
        FailingMap { map, backend, target, renderer, drawn: 0, failing: 0, _pool: pool }
    }

    /// Takes in the renderer's reports, then draws the map's latest update
    /// unless it drew it already, and returns whether it did. A still that
    /// hasn't loaded draws no frame.
    fn draw(&mut self) -> bool {
        self.map.process_events();
        let Some(update) = self.map.update_reader().latest().filter(|update| update.generation != self.drawn) else {
            return false;
        };
        self.drawn = update.generation;
        let gpu = self.backend.gpu();
        if let Some(frame) = self.renderer.update(gpu, &update.parameters) {
            let image = self.target.begin_frame(gpu).expect("the frame's image");
            let drawn = if self.failing > 0 {
                self.failing -= 1;
                self.renderer.render(&mut FailingDrapes { gpu }, &image, frame)
            } else {
                self.renderer.render(gpu, &image, frame)
            };
            let handoff = self.target.end_frame(gpu);
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
                let image = self.target.read_image(self.backend.gpu()).expect("the still's pixels");
                return Some(ended.map(|()| image.data));
            }
            if !self.draw() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        None
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

/// Asserts that a still ended with the error of a GPU that failed its
/// drapes.
fn assert_drapes_failed(ended: Option<Result<Vec<u8>, Error>>) {
    let error = ended.expect("the still never ended").expect_err("the still drew its drapes");
    assert_eq!(ErrorKind::Native, error.kind);
    assert!(error.message.contains("failed on purpose"), "{error}");
}

/// A still whose drapes fail to draw once, in its one frame, ends with the
/// GPU's error rather than an image without them, and the next still draws
/// them, as a new map's does.
#[test]
fn a_still_whose_drapes_fail_to_draw_once_ends_with_the_error() {
    let mut map = FailingMap::new(MapMode::Static);
    map.failing = 1;
    assert_drapes_failed(map.still());
    assert_eq!((0, 1), (map.failing, map.frames()));
    let image = map.still().expect("the next still never ended").expect("the next still");
    assert_eq!(2, map.frames());
    let new = FailingMap::new(MapMode::Static).still().expect("a new map's still never ended").unwrap();
    assert!(new == image, "a new map's still draws other pixels");
}

/// A still whose drapes fail to draw in every frame ends with the GPU's
/// error, in one frame, each time it is asked for.
#[test]
fn a_still_whose_drapes_fail_to_draw_in_every_frame_ends_with_the_error() {
    let mut map = FailingMap::new(MapMode::Static);
    map.failing = usize::MAX;
    assert_drapes_failed(map.still());
    assert_eq!(1, map.frames());
    assert_drapes_failed(map.still());
    assert_eq!(2, map.frames());
}

/// A continuous map whose drapes fail to draw reports the error with each
/// frame, and asks for frame after frame, until they draw, and then goes
/// idle.
#[test]
fn a_continuous_map_whose_drapes_fail_to_draw_asks_for_frames_until_they_draw() {
    let mut map = FailingMap::new(MapMode::Continuous);
    // Whether the map went idle, and the errors it reported.
    let drain = |map: &mut FailingMap| {
        let events = map.map.drain_events();
        let idle = events.iter().any(|event| matches!(event, MapEvent::Idle));
        (idle, events.iter().filter(|event| matches!(event, MapEvent::RenderError(_))).count())
    };
    map.failing = usize::MAX;
    let mut errors = 0;
    for _ in 0..2_000 {
        if errors >= 10 {
            break;
        }
        if !map.draw() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let (idle, reported) = drain(&mut map);
        assert!(!idle, "the map went idle while its drapes failed");
        errors += reported;
    }
    assert!(errors >= 10, "{errors} errors: the map stopped asking for frames");
    map.failing = 0;
    let idle = (0..2_000).any(|_| {
        if !map.draw() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let (idle, reported) = drain(&mut map);
        assert_eq!(0, reported, "an error once the drapes draw");
        idle
    });
    assert!(idle, "the map never went idle");
}

/// The fog lands once on each pixel, however many stacks drape the surface:
/// where the stack above draws nothing, the ground shows as it does without
/// that stack, and inside its opaque fill, as it does with the fill in the
/// stack below.
#[test]
fn fog_lands_once_where_stacks_overlap() {
    let (Some((above, _)), Some((none, _)), Some((below, _))) = (
        render(&fogged_style(Blue::Above), ViewportMode::Default),
        render(&fogged_style(Blue::None), ViewportMode::Default),
        render(&fogged_style(Blue::Below), ViewportMode::Default),
    ) else {
        return;
    };
    // Far ground east of the fill, where the stack above draws nothing.
    for (x, y) in [(210, 70), (200, 60)] {
        assert_eq!(pixel(&none, x, y), pixel(&above, x, y), "the ground at {x}, {y}");
    }
    // Far ground inside the fill.
    for (x, y) in [(40, 70), (30, 100)] {
        assert_ne!(pixel(&none, x, y), pixel(&above, x, y), "the fill at {x}, {y}");
        assert_eq!(pixel(&below, x, y), pixel(&above, x, y), "the fill at {x}, {y}");
    }
}
