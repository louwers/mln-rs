// A continuous map over terrain draws a drape again only when what it
// shows changed: its layers' values, the tiles or image they draw, their
// feature state, or, once the camera rests, the zoom, one tile a frame. A
// still draws them all at once. A hillshade, over
// terrain or not, prepares a DEM tile's slopes again only when its border
// changed.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use mln_gfx::RendererBackend as _;
use mln_gfx::frame::FrameTarget;
use mln_gfx::gpu::{
    BindingsDesc, BindingsId, BufferDesc, BufferId, BufferUsage, Caps, Draw, GeometryDesc, GeometryId, Gpu, GpuError,
    PassDesc, PipelineDesc, PipelineId, ReadbackId, ResourceId, SamplerId, ShaderId, ShaderKey, TextureDesc, TextureId,
    TextureRegion,
};
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map::transform::{TerrainGround, Transform};
use mln_map_types::{
    CameraLimits, CameraOptions, ConstrainMode, Elevation, MapDebugOptions, MapMode, TerrainSkirtLength, TileLodMode,
    ViewportMode,
};
use mln_storage::{FileSource, Resource, Responder, Response};
use mln_style::expression::expression::FeatureState;
use mln_style::sources::{GeoJSONOptions, GeoJsonSourceType, ImageSourceType, image_source};
use mln_style::{Edit, Style};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::geo::{LatLng, WrapMode};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use parking_lot::Mutex;

use crate::Registry;
use crate::renderer::Renderer;
use crate::renderer_event::{RenderMode, RendererEvent, RendererEvents};
use crate::tile::tile::TileOperation;
use crate::update_parameters::UpdateParameters;

const SIDE: u32 = 256;

/// A white map with a raster layer, a fill whose color follows the feature
/// state "hover" and whose opacity follows the global state "opacity", and a
/// line whose width follows the zoom, over a terrain of flat ground.
const STYLE: &str = r#"{
    "version": 8,
    "transition": {"duration": 0},
    "state": {"opacity": {"default": 1}},
    "terrain": {"source": "dem"},
    "sources": {
        "dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 10},
        "raster": {"type": "raster", "tiles": ["raster/{z}/{x}/{y}.png"], "tileSize": 256},
        "shapes": {
            "type": "geojson",
            "data": {
                "type": "Feature",
                "id": 1,
                "properties": {},
                "geometry": {"type": "Polygon", "coordinates": [[[-80, 50], [-70, 50], [-70, 60], [-80, 60], [-80, 50]]]}
            }
        }
    },
    "layers": [
        {"id": "background", "type": "background", "paint": {"background-color": "white"}},
        {"id": "raster", "type": "raster", "source": "raster", "paint": {"raster-opacity": 0.5}},
        {
            "id": "fill",
            "type": "fill",
            "source": "shapes",
            "paint": {
                "fill-color": ["case", ["boolean", ["feature-state", "hover"], false], "red", "blue"],
                "fill-opacity": ["global-state", "opacity"]
            }
        },
        {
            "id": "line",
            "type": "line",
            "source": "shapes",
            "paint": {"line-width": ["interpolate", ["linear"], ["zoom"], 0, 1, 10, 10]}
        }
    ]
}"#;

/// A PNG of `side` pixels a side, every pixel `pixel`.
fn png(side: u32, pixel: [u8; 4]) -> Response {
    let mut image = PremultipliedImage::new(Size::new(side, side));
    for texel in image.data.as_chunks_mut::<4>().0 {
        *texel = pixel;
    }
    let mut response = Response::new();
    response.data = Some(Arc::new(mln_image::encode_png(&image)));
    response
}

/// Answers every tile request at once: sea level for the DEM, or 25 km up if
/// `high`, and grey for the raster source, except a tile that waits while it
/// is held, that of the source whose URLs start with the held prefix.
#[derive(Default)]
struct Tiles {
    high: bool,
    held: Mutex<Option<(&'static str, CanonicalTileID)>>,
    waiting: Mutex<Vec<(Responder, Response)>>,
    /// The DEM requests answered, which stay open.
    dem_answered: Mutex<Vec<Responder>>,
}

impl FileSource for Tiles {
    fn load(&self, resource: &Resource, responder: Responder) {
        let data = resource.tile_data.as_ref().expect("only tiles are requested");
        let tile = CanonicalTileID::new(data.z as u8, data.x as u32, data.y as u32);
        let response = if data.url_template.starts_with("dem/") {
            // (1 · 65536 + 134 · 256 + 160) · 0.1 − 10000 = 0 m, and
            // (5 · 65536 + 87 · 256 + 48) · 0.1 − 10000 = 25000 m.
            png(256, if self.high { [5, 87, 48, 255] } else { [1, 134, 160, 255] })
        } else {
            png(256, [128, 128, 128, 255])
        };
        if self.held.lock().is_some_and(|(prefix, held)| held == tile && data.url_template.starts_with(prefix)) {
            self.waiting.lock().push((responder, response));
        } else {
            responder.send(response);
            if data.url_template.starts_with("dem/") {
                self.dem_answered.lock().push(responder);
            }
        }
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

impl Tiles {
    /// Answers the DEM requests that stay open again, 25 km up, as the file
    /// source does when their data expires and the server has new data.
    fn reload_dem_high(&self) {
        let answered = std::mem::take(&mut *self.dem_answered.lock());
        let open: Vec<Responder> = answered.into_iter().filter(|responder| !responder.is_cancelled()).collect();
        assert!(!open.is_empty(), "no DEM request stays open");
        for responder in &open {
            responder.send(png(256, [5, 87, 48, 255]));
        }
        self.dem_answered.lock().extend(open);
    }

    /// Answers the held tile.
    fn release(&self) {
        *self.held.lock() = None;
        for (responder, response) in std::mem::take(&mut *self.waiting.lock()) {
            responder.send(response);
        }
    }
}

/// What a [`Failing`] GPU fails.
#[derive(Clone, Copy)]
enum Fail {
    /// Beginning a drape's pass.
    DrapePasses,
    /// Making an index buffer.
    IndexBuffers,
}

/// A GPU that fails what `fail` says, as a device out of memory would, and
/// does the rest on `gpu`.
struct Failing<'a> {
    gpu: &'a mut dyn Gpu,
    fail: Fail,
}

/// The error of what a [`Failing`] GPU fails.
fn failed<T>() -> Result<T, GpuError> {
    Err(GpuError::Invalid("failed on purpose".to_owned()))
}

impl Gpu for Failing<'_> {
    fn caps(&self) -> &Caps {
        self.gpu.caps()
    }
    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        match (self.fail, desc.usage) {
            (Fail::IndexBuffers, BufferUsage::Index) => failed(),
            _ => self.gpu.create_buffer(desc),
        }
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
        if matches!(self.fail, Fail::DrapePasses) && desc.label == "drape" {
            return failed();
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

/// [`STYLE`], with what its sources' types know without a fetch.
fn style() -> Style {
    style_of(STYLE)
}

/// The style of `json`, with what its sources' types know without a fetch.
fn style_of(json: &str) -> Style {
    let registry = Registry::with_builtins();
    let parsed = Style::from_json(registry.style(), json, None).expect("the style");
    let mut style = Style::new(Arc::clone(registry.style()));
    for source in parsed.get_sources().iter() {
        style.apply(Edit::AddSource(crate::tests::ready(source.clone()))).unwrap();
    }
    for layer in parsed.get_layer_list().iter() {
        style.apply(Edit::AddLayer { layer: layer.clone(), before: None }).unwrap();
    }
    style.apply(Edit::SetTerrain(parsed.get_terrain().map(|terrain| (**terrain).clone()))).unwrap();
    style.apply(Edit::SetTransition(parsed.get_transition_options())).unwrap();
    style.apply(Edit::SetGlobalStateDefaults((**parsed.get_global_state_defaults()).clone())).unwrap();
    style
}

/// A map of [`style`] over WebGPU, drawn frame by frame, continuous unless
/// its `mode` makes each frame a still.
struct DrapedMap {
    mode: MapMode,
    backend: mln_webgpu::RendererBackend,
    target: OffscreenTarget<mln_webgpu::WebGpu>,
    renderer: Renderer,
    events: RendererEvents,
    tiles: Arc<Tiles>,
    style: Arc<Style>,
    transform: Transform,
    camera_moving: bool,
    /// Whether the last frame asked for another.
    needs_repaint: bool,
    /// Whether the last frame failed on the GPU.
    failed: bool,
    /// The ground the renderer last told of, which the updates carry, as a
    /// map's do.
    elevation: Option<Arc<dyn Elevation>>,
    /// The drapes' byte cap that each frame's drapes, and what they draw
    /// with, hold to.
    drape_byte_cap: Option<usize>,
    /// What the GPU fails in each frame's encode, if anything.
    fail: Option<Fail>,
    work: TileWork,
    _pool: mln_util::pool::Pool,
}

/// The tiles that the renderer requested and that weren't answered yet, and
/// those being parsed, as its events tell.
#[derive(Default)]
struct TileWork {
    unanswered: BTreeSet<(String, OverscaledTileID)>,
    parsing: BTreeSet<(String, OverscaledTileID)>,
}

impl TileWork {
    fn record(&mut self, op: TileOperation, tile: (String, OverscaledTileID)) {
        match op {
            TileOperation::RequestedFromCache | TileOperation::RequestedFromNetwork => {
                self.unanswered.insert(tile);
            }
            TileOperation::LoadFromCache | TileOperation::LoadFromNetwork => {
                self.unanswered.remove(&tile);
            }
            TileOperation::StartParse => {
                self.parsing.insert(tile);
            }
            TileOperation::EndParse => {
                self.parsing.remove(&tile);
            }
            TileOperation::Error | TileOperation::Cancelled => {
                self.unanswered.remove(&tile);
                self.parsing.remove(&tile);
            }
            TileOperation::NullOp => {}
        }
    }

    /// Whether the renderer waits for no tile but `held` unanswered ones.
    fn idle(&self, held: usize) -> bool {
        self.unanswered.len() == held && self.parsing.is_empty()
    }
}

impl DrapedMap {
    /// The map at zoom 2, seen from above, over four drapes.
    fn new(tiles: Tiles) -> Self {
        let device = mln_webgpu::Device::new(None).expect("device");
        let mut backend = mln_webgpu::RendererBackend::new(&device);
        let pool = crate::tests::pool();
        let mut renderer = Renderer::new(
            backend.gpu(),
            1.0,
            mln_text::local_glyph_rasterizer::LocalGlyphs::default(),
            Arc::new(|| {}),
            &pool,
        );
        let events = RendererEvents::new();
        renderer.set_event_queue(Some(events.clone()));
        let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(Size::new(SIDE, SIDE)).unwrap();
        transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(-10.0, 10.0).unwrap()),
            zoom: Some(2.0),
            ..Default::default()
        });
        DrapedMap {
            mode: MapMode::Continuous,
            backend,
            target: OffscreenTarget::new(Size::new(SIDE, SIDE)),
            renderer,
            events,
            tiles: Arc::new(tiles),
            style: Arc::new(style()),
            transform,
            camera_moving: false,
            needs_repaint: false,
            failed: false,
            elevation: None,
            drape_byte_cap: None,
            fail: None,
            work: TileWork::default(),
            _pool: pool,
        }
    }

    fn update_parameters(&self) -> UpdateParameters {
        let file_source: Arc<dyn FileSource> = Arc::<Tiles>::clone(&self.tiles);
        UpdateParameters {
            style_loaded: true,
            mode: self.mode,
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            time_point: None,
            view: self.transform.view().clone(),
            min_zoom: 0.0,
            max_zoom: 22.0,
            camera_moving: self.camera_moving,
            style: Arc::clone(&self.style),
            sprite_loaded: true,
            feature_states: Arc::default(),
            file_source: Some(file_source),
            prefetch_zoom_delta: 0,
            still_image_request: self.mode == MapMode::Static,
            still_image_request_id: 0,
            update_id: 0,
            cross_source_collisions: true,
            fast_pfor_enabled: false,
            terrain_skirt_length: TerrainSkirtLength::Auto,
            elevation: self.elevation.clone(),
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: mln_math::deg2rad(60.0),
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
        }
    }

    /// Draws a frame, and returns whether everything it shows had loaded and
    /// the tiles of the drapes of the first stack it drew.
    fn frame(&mut self) -> (bool, BTreeSet<OverscaledTileID>) {
        let (full, passes) = self.frame_passes();
        (full, passes.into_iter().filter(|&(_, stack)| stack == 0).map(|(tile, _)| tile).collect())
    }

    /// Draws a frame, and returns whether everything it shows had loaded and
    /// the tiles and stacks of the drapes it drew. A still that hasn't
    /// loaded draws nothing.
    fn frame_passes(&mut self) -> (bool, BTreeSet<(OverscaledTileID, usize)>) {
        let parameters = Arc::new(self.update_parameters());
        let gpu = self.backend.gpu();
        let frame = self.renderer.update(gpu, &parameters);
        assert!(frame.is_some() || self.mode == MapMode::Static, "a continuous map's frame");
        if let Some(frame) = frame {
            let image = self.target.begin_frame(gpu).expect("the frame's image");
            let drawn = match self.fail {
                Some(fail) => self.renderer.render(&mut Failing { gpu, fail }, &image, frame),
                None => self.renderer.render(gpu, &image, frame),
            };
            let handoff = self.target.end_frame(gpu);
            self.renderer.finish_frame(gpu, drawn, handoff);
        }
        let mut full = false;
        self.needs_repaint = false;
        self.failed = false;
        for event in self.events.take() {
            match event {
                RendererEvent::RenderFrameFinished { mode, needs_repaint, error, .. } => {
                    full = mode == RenderMode::Full;
                    self.needs_repaint = needs_repaint;
                    self.failed = error.is_some();
                }
                RendererEvent::TerrainElevation(elevation) => self.elevation = elevation,
                RendererEvent::TileAction { op, id, source_id } => self.work.record(op, (source_id, id)),
                _ => {}
            }
        }
        let passes = self.renderer.take_drape_passes();
        let unique: BTreeSet<(OverscaledTileID, usize)> = passes.iter().copied().collect();
        assert_eq!(unique.len(), passes.len(), "a drape drawn twice in a frame");
        if let Some(cap) = self.drape_byte_cap {
            let terrain = self.renderer.rendering_stats().terrain;
            assert!(drapes_held(self) <= cap, "{} > {cap}: {terrain:?}", drapes_held(self));
        }
        (full, unique)
    }

    /// Sets the drapes' byte cap, which every frame then holds to.
    fn set_drape_byte_cap(&mut self, cap: usize) {
        self.renderer.set_drape_byte_cap(cap);
        self.drape_byte_cap = Some(cap);
    }

    /// The number of draws of the last frame.
    fn draw_calls(&self) -> usize {
        usize::try_from(self.renderer.rendering_stats().num_draw_calls).unwrap()
    }

    /// Draws frames until the renderer waits for nothing but the held tile,
    /// and then one more. Returns the tiles of the drapes they drew.
    fn settle(&mut self) -> BTreeSet<OverscaledTileID> {
        let mut drawn = BTreeSet::new();
        for _ in 0..1000 {
            drawn.extend(self.frame().1);
            if self.work.idle(self.tiles.waiting.lock().len()) {
                drawn.extend(self.frame().1);
                return drawn;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the map never settled");
    }

    /// Settles the map with the camera standing on the ground it draws, as a
    /// map's camera does.
    fn settle_on_ground(&mut self) {
        for _ in 0..10 {
            self.settle();
            let Some(elevation) = &self.elevation else {
                return;
            };
            self.transform.set_terrain(Some(TerrainGround { elevation: Some(Arc::clone(elevation)), clamped: true }));
            if !self.transform.follow_terrain() {
                return;
            }
        }
        panic!("the camera never stood on the ground");
    }

    /// The last frame's pixel at `x`, `y`.
    fn pixel(&mut self, x: u32, y: u32) -> [u8; 4] {
        let image = self.target.read_image(self.backend.gpu()).expect("the frame's pixels");
        let start = ((y * image.size.width + x) * 4) as usize;
        image.data[start..start + 4].try_into().unwrap()
    }

    /// The tiles of the drapes of the last frame.
    fn drapes(&mut self) -> BTreeSet<OverscaledTileID> {
        self.renderer.drape_tiles().into_iter().collect()
    }

    fn edit(&mut self, edit: Edit) {
        let mut style = (*self.style).clone();
        style.apply(edit).unwrap();
        self.style = Arc::new(style);
    }
}

impl Drop for DrapedMap {
    fn drop(&mut self) {
        let gpu = self.backend.gpu();
        self.target.tear_down(gpu);
    }
}

/// The map, settled, and its four drapes.
fn settled(tiles: Tiles) -> (DrapedMap, BTreeSet<OverscaledTileID>) {
    let mut map = DrapedMap::new(tiles);
    map.settle();
    let drapes = map.drapes();
    assert_eq!(4, drapes.len(), "drapes: {drapes:?}");
    (map, drapes)
}

#[test]
fn a_map_at_rest_draws_no_drape_again() {
    let (mut map, drapes) = settled(Tiles::default());
    let (full, passes) = map.frame();
    assert!(full);
    assert!(passes.is_empty(), "drapes drawn again: {passes:?}");
    let (_, passes) = map.frame();
    assert!(passes.is_empty(), "drapes drawn again: {passes:?}");
    assert_eq!(drapes, map.drapes());
}

/// With every draped layer in one stack, a map at rest draws the surface once
/// for each terrain tile and nothing else.
#[test]
fn a_map_of_one_stack_at_rest_draws_each_terrain_tile_once() {
    let (mut map, drapes) = settled(Tiles::default());
    let (full, passes) = map.frame();
    assert!(full);
    assert!(passes.is_empty(), "drapes drawn again: {passes:?}");
    assert_eq!(drapes.len(), map.draw_calls());
}

/// A layer of `json`, a style's, with its sources added to `map`'s style.
fn add_layer(map: &mut DrapedMap, json: &str, before: Option<&str>) {
    let parsed = Style::from_json(Registry::with_builtins().style(), json, None).expect("the style");
    for source in parsed.get_sources().iter() {
        map.edit(Edit::AddSource(crate::tests::ready(source.clone())));
    }
    let layer = parsed.get_layer_list().iter().next().expect("the layer").clone();
    map.edit(Edit::AddLayer { layer, before: before.map(str::to_owned) });
}

/// A circle at one point, in the north-west drape's tile.
const CIRCLE: &str = r#"{
    "version": 8,
    "sources": {
        "point": {"type": "geojson", "data": {"type": "Point", "coordinates": [-75, 55]}}
    },
    "layers": [{"id": "circle", "type": "circle", "source": "point"}]
}"#;

/// [`STYLE`] with a circle over its background, which splits its layers
/// into two stacks that both draw in every tile, settled, and its four
/// drapes.
fn two_stacks() -> (DrapedMap, BTreeSet<OverscaledTileID>) {
    let (mut map, drapes) = settled(Tiles::default());
    add_layer(&mut map, CIRCLE, Some("raster"));
    let both: BTreeSet<(OverscaledTileID, usize)> = drapes.iter().flat_map(|&tile| [(tile, 0), (tile, 1)]).collect();
    let mut drawn = BTreeSet::new();
    for _ in 0..1000 {
        drawn.extend(map.frame_passes().1);
        if drawn == both {
            break;
        }
    }
    assert_eq!(both, drawn, "both stacks' drapes drawn");
    map.settle();
    (map, drapes)
}

/// A map of two stacks at rest draws the surface once for each terrain tile
/// and stack, and the circle between them.
#[test]
fn a_map_of_two_stacks_at_rest_draws_each_terrain_tile_once_a_stack() {
    let (mut map, drapes) = two_stacks();
    let (full, passes) = map.frame_passes();
    assert!(full);
    assert!(passes.is_empty(), "drapes drawn again: {passes:?}");
    assert_eq!(2 * drapes.len() + 1, map.draw_calls());
}

/// A change of a layer in the second stack draws only that stack's drapes
/// again.
#[test]
fn a_paint_change_in_a_stack_draws_only_its_drapes_again() {
    let (mut map, drapes) = two_stacks();
    map.edit(Edit::SetLayerProperty {
        layer: "line".to_owned(),
        name: "line-color".to_owned(),
        value: serde_json::json!("green"),
    });
    let second: BTreeSet<(OverscaledTileID, usize)> = drapes.iter().map(|&tile| (tile, 1)).collect();
    assert_eq!(second, map.frame_passes().1);
    assert!(map.frame_passes().1.is_empty(), "the drapes are drawn again once only");
}

/// A sky whose fog of `fog_color` covers the ground from the near end of its
/// depth.
fn sky(fog_color: &str) -> Edit {
    let json = format!(
        r#"{{"version": 8, "sources": {{}}, "layers": [], "sky": {{"fog-color": "{fog_color}", "fog-ground-blend": 0}}}}"#
    );
    let parsed = Style::from_json(Registry::with_builtins().style(), &json, None).expect("the style");
    Edit::SetSky(parsed.get_sky().map(|sky| (**sky).clone()))
}

/// The fog is in no drape's key: on a map of two stacks pitched far enough
/// to fog, a change of the fog's color draws no drape again, and the far
/// ground shows it.
#[test]
fn a_fog_change_draws_no_drape_again() {
    let (mut map, _) = two_stacks();
    map.edit(sky("red"));
    // Zoomed in, the ground reaches the horizon.
    let limits = CameraLimits { max_pitch: mln_math::deg2rad(85.0), ..*map.transform.state().limits() };
    map.transform.set_limits(limits).unwrap();
    map.transform.jump_to(&CameraOptions { zoom: Some(4.0), pitch: Some(75.0), ..Default::default() });
    map.settle();
    frames_until_full(&mut map);
    assert_eq!(2, map.renderer.rendering_stats().terrain.stacks);
    // Far ground, just under the horizon.
    let horizon = map.transform.view().mercator_horizon();
    let y = (f64::from(SIDE / 2) - horizon + 4.0) as u32;
    let red = map.pixel(SIDE / 2, y);

    map.edit(sky("blue"));
    let (full, passes) = map.frame_passes();
    assert!(full);
    assert!(passes.is_empty(), "drapes drawn again: {passes:?}");
    let blue = map.pixel(SIDE / 2, y);
    assert!(red[0] > red[2] && blue[2] > blue[0], "the far ground under a red fog {red:?}, then a blue one {blue:?}");
}

/// [`STYLE`] with a circle under its line, which shows in the north-west
/// drape's tile only: the line's stack draws nothing in the other three.
#[test]
fn a_stack_has_no_drape_where_it_draws_nothing() {
    let (mut map, drapes) = settled(Tiles::default());
    add_layer(&mut map, CIRCLE, Some("line"));
    map.settle();
    let _north_west = OverscaledTileID::new(2, 0, CanonicalTileID::new(2, 1, 1));
    let (full, passes) = map.frame_passes();
    assert!(full);
    assert!(passes.is_empty(), "drapes drawn again: {passes:?}");
    let terrain = map.renderer.rendering_stats().terrain;
    assert_eq!(drapes.len() + 1, terrain.drapes, "the line's stack has the north-west drape only");
    // The first stack's surface, the line's in the north-west tile, and the
    // circle.
    assert_eq!(drapes.len() + 1, terrain.surface_draw_calls);
    assert_eq!(drapes.len() + 2, map.draw_calls());

    // The shapes move out of the north-west tile into the south-west one:
    // the line's drape there is made, and the north-west one goes.
    let geojson: geojson::GeoJson = r#"{
        "type": "Feature",
        "id": 1,
        "properties": {},
        "geometry": {"type": "Polygon", "coordinates": [[[-80, -50], [-70, -50], [-70, -40], [-80, -40], [-80, -50]]]}
    }"#
    .parse()
    .unwrap();
    let data = mln_style::sources::geojson::create_geojson_data(geojson, &GeoJSONOptions::default_options()).unwrap();
    map.edit(GeoJsonSourceType::set_data("shapes", Some(data)));
    let mut drawn = BTreeSet::new();
    for _ in 0..1000 {
        drawn.extend(map.frame_passes().1);
        if drawn.iter().any(|&(_, stack)| stack == 1) && map.work.idle(0) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    drawn.extend(map.frame_passes().1);
    let south_west = OverscaledTileID::new(2, 0, CanonicalTileID::new(2, 1, 2));
    let line: BTreeSet<_> = drawn.iter().filter(|&&(_, stack)| stack == 1).collect();
    assert_eq!(BTreeSet::from([&(south_west, 1)]), line);
    map.frame_passes();
    let terrain = map.renderer.rendering_stats().terrain;
    assert_eq!(drapes.len() + 1, terrain.drapes, "the line's stack has the south-west drape only");
    assert_eq!(drapes.len() + 1, terrain.surface_draw_calls);
}

/// A raster layer of its own stack, whose tile of the north-west drape is
/// held: the stack has no drape there until the tile arrives, and then
/// draws it once.
#[test]
fn a_stack_gets_a_drape_where_its_tile_arrives() {
    let north_west = CanonicalTileID::new(2, 1, 1);
    let tiles = Tiles { held: Mutex::new(Some(("late/", north_west))), ..Tiles::default() };
    let (mut map, drapes) = settled(tiles);
    add_layer(&mut map, CIRCLE, None);
    add_layer(
        &mut map,
        r#"{
            "version": 8,
            "sources": {"late": {"type": "raster", "tiles": ["late/{z}/{x}/{y}.png"], "tileSize": 512}},
            "layers": [{"id": "late", "type": "raster", "source": "late", "paint": {"raster-fade-duration": 0}}]
        }"#,
        None,
    );
    map.settle();
    assert_eq!(1, map.tiles.waiting.lock().len(), "the held tile was requested");
    let (full, passes) = map.frame_passes();
    assert!(!full, "the held tile hasn't loaded");
    assert!(passes.is_empty(), "drapes drawn again: {passes:?}");
    let terrain = map.renderer.rendering_stats().terrain;
    assert_eq!(2 * drapes.len() - 1, terrain.drapes, "no drape of the raster's stack in the north-west");
    assert_eq!(2 * drapes.len() - 1, terrain.surface_draw_calls);

    map.tiles.release();
    let mut drawn = BTreeSet::new();
    for _ in 0..1000 {
        drawn.extend(map.frame_passes().1);
        if map.work.idle(0) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    drawn.extend(map.frame_passes().1);
    let north_west = OverscaledTileID::new(2, 0, north_west);
    assert_eq!(BTreeSet::from([(north_west, 1)]), drawn);
    let terrain = map.renderer.rendering_stats().terrain;
    assert_eq!(2 * drapes.len(), terrain.drapes);
    assert_eq!(2 * drapes.len(), terrain.surface_draw_calls);
}

/// The first stack draws the surface of every tile, for the depth the
/// stacks above test, though it has a drape only where it draws something.
#[test]
fn the_first_stack_draws_every_tile_without_a_drape_where_it_draws_nothing() {
    let (mut map, drapes) = settled(Tiles::default());
    for layer in ["background", "raster"] {
        map.edit(Edit::RemoveLayer(layer.to_owned()));
    }
    map.settle();
    let terrain = map.renderer.rendering_stats().terrain;
    assert_eq!(1, terrain.drapes, "the shapes show in the north-west drape only");
    assert_eq!(drapes.len(), terrain.surface_draw_calls);
}

/// The rendering stats count what a map of two stacks holds of the terrain,
/// and what each frame drew of it: here, the drapes of one stack, and then,
/// at rest, none.
#[test]
fn the_rendering_stats_count_the_terrain() {
    let (mut map, drapes) = two_stacks();
    map.edit(Edit::SetLayerProperty {
        layer: "line".to_owned(),
        name: "line-color".to_owned(),
        value: serde_json::json!("green"),
    });
    map.frame_passes();
    let stats = map.renderer.rendering_stats();
    let tiles = drapes.len();
    // The DEM's tiles of 256 pixels make terrain tiles of 512, and drapes
    // of 1024 texels a side, with their mips.
    let texels = 1024 * 1024;
    let terrain = stats.terrain;
    assert_eq!(2, terrain.stacks);
    assert_eq!(2 * tiles, terrain.drapes);
    assert_eq!(2 * tiles * (4 * texels + 4 * texels / 3), terrain.mem_drapes);
    assert_eq!(4 * texels, terrain.mem_drape_depth_stencil);
    assert_eq!(0, terrain.mem_drape_composite, "no layer composites");
    assert_eq!(tiles, terrain.drapes_drawn);
    assert_eq!(tiles, terrain.drape_passes);
    assert!(terrain.drape_draw_calls >= tiles, "{terrain:?}");
    assert_eq!(2 * tiles, terrain.surface_draw_calls);
    assert!(stats.num_mipmap_generations >= tiles as i32, "{stats:?}");
    assert!(stats.num_render_passes > tiles as i32, "{stats:?}");
    // Each DEM image has a border of a pixel.
    let dem = 258 * 258 * 4;
    assert!(terrain.mem_dem_textures >= dem, "{terrain:?}");
    assert!(terrain.mem_dem_data >= dem && terrain.mem_dem_data.is_multiple_of(dem), "{terrain:?}");
    assert!(terrain.mem_height_ranges > 0);

    map.frame_passes();
    let at_rest = map.renderer.rendering_stats().terrain;
    assert_eq!((0, 0, 0), (at_rest.drapes_drawn, at_rest.drape_passes, at_rest.drape_draw_calls));
    assert_eq!(2 * tiles, at_rest.surface_draw_calls);
    assert_eq!(terrain.mem_drapes, at_rest.mem_drapes);
}

/// The bytes of a drape of `side` texels a side, with its mips.
fn drape_bytes(side: usize) -> usize {
    4 * side * side + 4 * side * side / 3
}

/// The sides of the last frame's drapes, by tile.
fn sides(map: &mut DrapedMap) -> BTreeSet<(OverscaledTileID, u32)> {
    map.renderer.drape_sizes().into_iter().collect()
}

/// A byte cap under what the drapes would hold lowers the sides of some,
/// which draw again once, smaller; every tile keeps its drape. Raised again
/// while the camera moves, it leaves them as they are until the camera
/// rests, and then they draw again once at their whole side, one a frame.
#[test]
fn a_byte_cap_lowers_the_sides_of_some_drapes_which_draw_again_once() {
    let (mut map, drapes) = settled(Tiles::default());
    let whole: BTreeSet<_> = drapes.iter().map(|&tile| (tile, 1024)).collect();
    assert_eq!(whole, sides(&mut map), "the tiles show at 512 pixels, at two texels a pixel");

    let cap = 3 * drape_bytes(1024);
    map.set_drape_byte_cap(cap);
    let (_, passes) = map.frame();
    let lowered: BTreeSet<_> = sides(&mut map).into_iter().filter(|&(_, side)| side < 1024).collect();
    assert!(!lowered.is_empty(), "{:?}", sides(&mut map));
    assert!(lowered.iter().all(|&(_, side)| side == 512), "a step down: {lowered:?}");
    assert_eq!(lowered.iter().map(|&(tile, _)| tile).collect::<BTreeSet<_>>(), passes, "the lowered drapes draw again");
    let terrain = map.renderer.rendering_stats().terrain;
    assert_eq!(drapes.len(), terrain.drapes, "every tile keeps its drape");
    assert_eq!(drapes.len(), terrain.surface_draw_calls);
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");

    map.camera_moving = true;
    map.set_drape_byte_cap(usize::MAX);
    assert!(map.frame().1.is_empty(), "drapes drawn larger while the camera moves");
    map.camera_moving = false;
    let lowered: BTreeSet<_> = lowered.iter().map(|&(tile, _)| tile).collect();
    assert_one_a_frame(&lowered, &frames_until_full(&mut map));
    assert_eq!(whole, sides(&mut map));
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
}

/// While the camera moves, a drape larger than the cap allows draws again
/// smaller at once, so that the drapes hold no more than the cap.
#[test]
fn a_byte_cap_holds_while_the_camera_moves() {
    let (mut map, drapes) = settled(Tiles::default());
    map.camera_moving = true;
    let cap = 3 * drape_bytes(1024);
    map.set_drape_byte_cap(cap);
    let (_, passes) = map.frame();
    assert!(!passes.is_empty());
    let terrain = map.renderer.rendering_stats().terrain;
    assert_eq!(drapes.len(), terrain.drapes);
}

/// The bytes of the depth and stencil that the drapes of `side` texels a
/// side draw with.
fn depth_stencil_bytes(side: usize) -> usize {
    4 * side * side
}

/// The bytes the last frame's drapes held, with what they draw with.
fn drapes_held(map: &DrapedMap) -> usize {
    let terrain = map.renderer.rendering_stats().terrain;
    terrain.mem_drapes + terrain.mem_drape_depth_stencil + terrain.mem_drape_composite
}

/// [`STYLE`] with a circle under its line, which splits the line into a
/// stack of its own that draws in the north-west drape's tile only, settled
/// under a byte cap that holds the five drapes with a texture at their
/// whole side, and the four tiles.
fn sparse_stack_under_a_cap() -> (DrapedMap, BTreeSet<OverscaledTileID>) {
    let (mut map, drapes) = settled(Tiles::default());
    map.set_drape_byte_cap(5 * drape_bytes(1024) + depth_stencil_bytes(1024));
    add_layer(&mut map, CIRCLE, Some("line"));
    map.settle();
    frames_until_full(&mut map);
    let whole: BTreeSet<_> = drapes.iter().map(|&tile| (tile, 1024)).collect();
    assert_eq!(whole, sides(&mut map));
    assert_eq!(drapes.len() + 1, map.renderer.rendering_stats().terrain.drapes);
    (map, drapes)
}

/// The cap counts only the drapes that hold a texture: once a gesture ends,
/// the drapes of a stack that draws nothing in their tiles, stale by the
/// zoom, take no bytes from the others, which keep their sides and draw
/// again one tile a frame.
#[test]
fn a_byte_cap_counts_no_drape_of_a_stack_that_draws_nothing_there() {
    let (mut map, drapes) = sparse_stack_under_a_cap();
    let whole = sides(&mut map);
    map.camera_moving = true;
    map.transform.jump_to(&CameraOptions { zoom: Some(2.05), ..Default::default() });
    assert!(map.frame().1.is_empty(), "drapes drawn while the camera moves");
    map.camera_moving = false;
    let mut frames = Vec::new();
    for _ in 0..100 {
        let (full, drawn) = map.frame();
        assert_eq!(whole, sides(&mut map), "the sides at rest");
        frames.push(drawn);
        if full {
            break;
        }
    }
    assert_one_a_frame(&drapes, &frames);
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
}

/// Draws stills until one draws, which is complete, and returns the sides
/// of its drapes, by tile, and its pixels. A still that hasn't loaded draws
/// nothing.
fn still(map: &mut DrapedMap) -> (BTreeSet<(OverscaledTileID, u32)>, Vec<u8>) {
    assert_eq!(MapMode::Static, map.mode);
    for _ in 0..1000 {
        if map.frame().0 {
            let image = map.target.read_image(map.backend.gpu()).expect("the still's pixels");
            return (sides(map), image.data.clone());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("no still drew");
}

/// A still draws its drapes at the sides the cap gives the drapes that hold
/// a texture once it has drawn them, at once: a new stack that draws in one
/// tile only takes no bytes from the others' drapes, and the same still
/// drawn again draws the same.
#[test]
fn a_still_draws_its_drapes_at_their_sides_under_a_byte_cap_at_once() {
    let mut map = DrapedMap::new(Tiles::default());
    map.mode = MapMode::Static;
    map.settle();
    let drapes = map.drapes();
    map.set_drape_byte_cap(5 * drape_bytes(1024) + depth_stencil_bytes(1024));
    add_layer(&mut map, CIRCLE, Some("line"));
    let (first, image) = still(&mut map);
    let whole: BTreeSet<_> = drapes.iter().map(|&tile| (tile, 1024)).collect();
    assert_eq!(whole, first);
    assert_eq!(drapes.len() + 1, map.renderer.rendering_stats().terrain.drapes);
    let (second, again) = still(&mut map);
    assert_eq!(first, second, "the same still draws the same sides");
    assert!(image == again, "the same still draws the same pixels");
}

/// Drapes of a stack that stop drawing in most tiles leave room under the
/// cap for the drapes of the other stack in the tiles that draw: they draw
/// again larger in the same frame, though their key didn't change, as a
/// new map draws them.
#[test]
fn the_drapes_kept_in_a_tile_that_draws_take_its_new_side_at_once() {
    let cap = 5 * drape_bytes(1024) + depth_stencil_bytes(1024);
    let (mut map, drapes) = two_stacks();
    map.set_drape_byte_cap(cap);
    frames_until_full(&mut map);
    let lowered: BTreeSet<_> =
        sides(&mut map).into_iter().filter(|&(_, side)| side < 1024).map(|(tile, _)| tile).collect();
    assert!(!lowered.is_empty(), "{:?}", sides(&mut map));
    // The raster's stack, which draws in every tile, then draws its line in
    // the north-west one only.
    map.edit(Edit::RemoveLayer("raster".to_owned()));
    let (full, passes) = map.frame_passes();
    assert!(full);
    let whole: BTreeSet<_> = drapes.iter().map(|&tile| (tile, 1024)).collect();
    assert_eq!(whole, sides(&mut map));
    let first: BTreeSet<_> = passes.iter().filter(|&&(_, stack)| stack == 0).map(|&(tile, _)| tile).collect();
    assert_eq!(lowered, first, "the first stack's lowered drapes draw again");
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
    let image = map.target.read_image(map.backend.gpu()).expect("the frame's pixels").data;

    let mut new = DrapedMap::new(Tiles::default());
    new.set_drape_byte_cap(cap);
    new.style = Arc::clone(&map.style);
    new.settle();
    frames_until_full(&mut new);
    assert_eq!(whole, sides(&mut new));
    let new_image = new.target.read_image(new.backend.gpu()).expect("the frame's pixels").data;
    assert!(new_image == image, "a new map draws other pixels");
}

/// The cap counts, with the drapes' textures, the depth and stencil of each
/// side and the layer composite's textures in a frame that composites, at
/// rest and while the camera moves.
#[test]
fn a_byte_cap_holds_with_what_the_drapes_draw_with() {
    let (mut map, drapes) = settled(Tiles::default());
    let cap = 3 * drape_bytes(1024);
    map.set_drape_byte_cap(cap);
    map.edit(Edit::SetLayerProperty {
        layer: "fill".to_owned(),
        name: "fill-layer-opacity".to_owned(),
        value: serde_json::json!(0.5),
    });
    assert_eq!(drapes, map.frame().1);
    let terrain = map.renderer.rendering_stats().terrain;
    assert!(terrain.mem_drape_composite > 0, "the drapes composite");
    assert_eq!(drapes.len(), terrain.drapes, "every tile keeps its drape");

    map.camera_moving = true;
    map.transform.jump_to(&CameraOptions { zoom: Some(2.05), ..Default::default() });
    map.frame();
    map.set_drape_byte_cap(drape_bytes(1024));
    assert!(!map.frame().1.is_empty(), "the drapes draw again smaller");
    map.camera_moving = false;
    frames_until_full(&mut map);
    assert_eq!(drapes.len(), map.renderer.rendering_stats().terrain.drapes, "every tile keeps its drape");
}

/// A thick line in the corner of the north-west drape's tile that the view
/// shows, shifted 20 pixels to the right on screen, so that it draws by the
/// bearing.
const SHIFTED_LINE: &str = r#"{
    "version": 8,
    "sources": {
        "shifted": {"type": "geojson", "data": {"type": "LineString", "coordinates": [[-6, 2], [-6, 10]]}}
    },
    "layers": [{
        "id": "shifted",
        "type": "line",
        "source": "shifted",
        "paint": {"line-width": 8, "line-translate": [20, 0], "line-translate-anchor": "viewport"}
    }]
}"#;

/// Sets the color of [`STYLE`]'s line.
fn line_color(map: &mut DrapedMap, color: &str) {
    map.edit(Edit::SetLayerProperty {
        layer: "line".to_owned(),
        name: "line-color".to_owned(),
        value: serde_json::json!(color),
    });
}

/// While the camera moves, the drapes stale by the bearing keep the
/// drawings they were drawn from, though a byte cap would lower their
/// sides for drapes that then draw nothing: once back at that bearing and
/// drawn again larger, they draw what a new map draws.
#[test]
fn a_gesture_keeps_the_drawings_of_the_drapes_it_leaves_stale() {
    // Five drapes at 512 texels, then at 1024.
    let small = 5 * drape_bytes(512) + depth_stencil_bytes(512);
    let whole = 5 * drape_bytes(1024) + depth_stencil_bytes(1024);
    let (mut map, drapes) = settled(Tiles::default());
    // A first stack that draws in every tile, the shifted line in the
    // north-west one, and a second of the line only, which draws there
    // only.
    add_layer(&mut map, SHIFTED_LINE, Some("line"));
    add_layer(&mut map, CIRCLE, Some("line"));
    map.set_drape_byte_cap(small);
    map.settle();
    frames_until_full(&mut map);
    assert_eq!(drapes.iter().map(|&tile| (tile, 512)).collect::<BTreeSet<_>>(), sides(&mut map));
    assert_eq!(drapes.len() + 1, map.renderer.rendering_stats().terrain.drapes);

    // A gesture turns the map while the line's drapes draw again.
    map.camera_moving = true;
    map.transform.jump_to(&CameraOptions { bearing: Some(90.0), ..Default::default() });
    line_color(&mut map, "green");
    map.frame();
    map.transform.jump_to(&CameraOptions { bearing: Some(0.0), ..Default::default() });
    map.frame();
    // At rest, back at the bearing they were drawn at, a higher cap gives
    // the drapes their whole side while the line's draw again.
    map.camera_moving = false;
    map.set_drape_byte_cap(whole);
    line_color(&mut map, "blue");
    frames_until_full(&mut map);
    assert_eq!(drapes.iter().map(|&tile| (tile, 1024)).collect::<BTreeSet<_>>(), sides(&mut map));
    let image = map.target.read_image(map.backend.gpu()).expect("the frame's pixels").data;

    let mut new = DrapedMap::new(Tiles::default());
    new.set_drape_byte_cap(whole);
    new.style = Arc::clone(&map.style);
    new.settle();
    frames_until_full(&mut new);
    assert_eq!(sides(&mut map), sides(&mut new));
    let new_image = new.target.read_image(new.backend.gpu()).expect("the frame's pixels").data;
    assert!(new_image == image, "a new map draws other pixels");
}

/// Drapes stale by the zoom whose layer composites at the new zoom: as each
/// draws again in its turn and takes a layer composite, the others, which
/// wait for theirs, draw again smaller at once from what they show, so that
/// no frame holds more than the cap.
#[test]
fn a_byte_cap_holds_while_drapes_drawn_in_turn_start_to_composite() {
    let cap = 4 * drape_bytes(1024) + depth_stencil_bytes(1024);
    let (mut map, drapes) = settled(Tiles::default());
    map.set_drape_byte_cap(cap);
    add_layer(
        &mut map,
        r#"{
            "version": 8,
            "sources": {
                "veil": {
                    "type": "geojson",
                    "data": {"type": "Polygon", "coordinates": [[[-170, -80], [170, -80], [170, 80], [-170, 80], [-170, -80]]]}
                }
            },
            "layers": [{
                "id": "veil",
                "type": "fill",
                "source": "veil",
                "paint": {
                    "fill-color": "red",
                    "fill-opacity": 0.2,
                    "fill-layer-opacity": ["interpolate", ["linear"], ["zoom"], 2, 1, 2.05, 0.5]
                }
            }]
        }"#,
        Some("line"),
    );
    map.settle();
    frames_until_full(&mut map);
    assert_eq!(drapes.iter().map(|&tile| (tile, 1024)).collect::<BTreeSet<_>>(), sides(&mut map));
    assert_eq!(0, map.renderer.rendering_stats().terrain.mem_drape_composite, "no drape composites");

    map.camera_moving = true;
    map.transform.jump_to(&CameraOptions { zoom: Some(2.05), ..Default::default() });
    assert!(map.frame().1.is_empty(), "drapes drawn while the camera moves");
    map.camera_moving = false;
    frames_until_full(&mut map);
    assert!(map.renderer.rendering_stats().terrain.mem_drape_composite > 0, "the drapes composite");
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
}

/// The drapes that wait to grow keep their textures at their old side, which
/// no drape draws at any more, and the cap, which counts them at their new
/// side, still holds, with the depth and stencil of the side drawn at only.
/// Here a DEM of 300-pixel tiles makes drapes of at most 1200 texels a side,
/// no power of two, so a far tile's 1024 is little below it: the bytes a
/// drape's 1024 texels save on 1200 are less than half a depth and stencil
/// of 1024.
#[test]
fn a_byte_cap_holds_while_drapes_wait_to_grow_alone_at_their_side() {
    let mut map = DrapedMap::new(Tiles::default());
    map.style = Arc::new(style_of(&STYLE.replace(r#""tileSize": 256, "maxzoom""#, r#""tileSize": 300, "maxzoom""#)));
    map.transform.jump_to(&CameraOptions { zoom: Some(1.0), pitch: Some(60.0), ..Default::default() });
    map.settle();
    frames_until_full(&mut map);
    let tile = |x, y| OverscaledTileID::new(1, 0, CanonicalTileID::new(1, x, y));
    let (near, far) = ([tile(0, 1), tile(1, 1)], [tile(0, 0), tile(1, 0)]);
    let pitched: BTreeSet<_> = near.map(|tile| (tile, 1200)).into_iter().chain(far.map(|tile| (tile, 1024))).collect();
    assert_eq!(pitched, sides(&mut map), "the far tiles show at 481 pixels");

    // Zoomed in, the far tiles show at 515 pixels, and their drapes wait
    // until the camera rests to grow, in turn.
    map.camera_moving = true;
    map.set_drape_byte_cap(4 * drape_bytes(1200) + depth_stencil_bytes(1200));
    map.transform.jump_to(&CameraOptions { zoom: Some(1.1), ..Default::default() });
    assert!(map.frame().1.is_empty(), "drapes drawn while the camera moves");
    let whole: BTreeSet<_> = near.iter().chain(&far).map(|&tile| (tile, 1200)).collect();
    assert_eq!(whole, sides(&mut map));
    let terrain = map.renderer.rendering_stats().terrain;
    assert_eq!(depth_stencil_bytes(1200), terrain.mem_drape_depth_stencil, "{terrain:?}");
    map.camera_moving = false;
    map.settle();
    frames_until_full(&mut map);
    assert_eq!(whole, sides(&mut map));
}

/// A frame in which drapes fail to draw, drawn from their key or again at a
/// new side only, fails, and the next draws them.
#[test]
fn a_frame_whose_drapes_fail_to_draw_fails_and_the_next_draws_them() {
    let (mut map, drapes) = settled(Tiles::default());
    let fails_then_draws = |map: &mut DrapedMap| {
        map.fail = Some(Fail::DrapePasses);
        let (full, drawn) = map.frame();
        assert!(drawn.is_empty(), "drapes drawn: {drawn:?}");
        assert!(!full && map.failed, "a frame whose drapes failed to draw is done");
        map.fail = None;
        let (full, drawn) = map.frame();
        assert!(full && !map.needs_repaint);
        drawn
    };
    line_color(&mut map, "green");
    assert_eq!(drapes, fails_then_draws(&mut map));
    map.set_drape_byte_cap(3 * drape_bytes(1024));
    let lowered: BTreeSet<_> =
        fails_then_draws(&mut map).into_iter().filter(|&tile| sides(&mut map).contains(&(tile, 512))).collect();
    assert!(!lowered.is_empty(), "{:?}", sides(&mut map));
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
}

/// A draw whose indexes failed to upload fails, in any build, and draws
/// once they upload: here the surface's, and those of the drapes, which
/// keep no key of the frames they failed in and draw again.
#[test]
fn a_draw_whose_indexes_failed_to_upload_fails_and_draws_once_they_do() {
    let mut map = DrapedMap::new(Tiles::default());
    map.fail = Some(Fail::IndexBuffers);
    map.settle();
    map.fail = None;
    map.settle();
    frames_until_full(&mut map);
    let image = map.target.read_image(map.backend.gpu()).expect("the frame's pixels").data;
    let mut new = DrapedMap::new(Tiles::default());
    new.style = Arc::clone(&map.style);
    new.settle();
    frames_until_full(&mut new);
    let new_image = new.target.read_image(new.backend.gpu()).expect("the frame's pixels").data;
    assert!(new_image == image, "a new map draws other pixels");
}

/// [`STYLE`]'s draped layers, removed, and a circle in their place.
fn only_a_circle(map: &mut DrapedMap) {
    for layer in ["background", "raster", "fill", "line"] {
        map.edit(Edit::RemoveLayer(layer.to_owned()));
    }
    add_layer(map, CIRCLE, None);
}

/// Without a draped layer, a map over terrain has no drape and draws no
/// surface: only its circle. The surface lets go of its draws and grid.
#[test]
fn a_map_without_a_draped_layer_draws_no_surface() {
    let (mut map, _) = settled(Tiles::default());
    assert_eq!((true, true), map.renderer.terrain_surface_holds());
    only_a_circle(&mut map);
    let drawn = map.settle();
    assert!(drawn.is_empty(), "drapes drawn: {drawn:?}");
    let (full, passes) = map.frame_passes();
    assert!(full);
    assert!(passes.is_empty(), "drapes drawn: {passes:?}");
    assert_eq!(4, map.drapes().len(), "terrain still covers the view");
    assert_eq!(1, map.draw_calls());
    assert!(!map.renderer.terrain_surface_holds().1, "the surface holds draws or its grid");
}

/// A map over terrain that never had a draped layer makes nothing for the
/// surface it doesn't draw: no shader or grid.
#[test]
fn a_map_that_never_drapes_makes_no_surface() {
    let mut map = DrapedMap::new(Tiles::default());
    only_a_circle(&mut map);
    map.settle();
    assert!(map.frame().0);
    assert_eq!(4, map.drapes().len(), "terrain covers the view");
    assert_eq!((false, false), map.renderer.terrain_surface_holds());
}

#[test]
fn a_paint_change_draws_every_drape_again() {
    let (mut map, drapes) = settled(Tiles::default());
    map.edit(Edit::SetLayerProperty {
        layer: "line".to_owned(),
        name: "line-color".to_owned(),
        value: serde_json::json!("green"),
    });
    assert_eq!(drapes, map.frame().1);
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
}

/// A layer drawn as a whole in the drapes keeps them at rest, as any other
/// layer does, and a change of its layer opacity draws them again.
#[test]
fn a_composited_layer_keeps_its_drapes_at_rest() {
    let fill_layer_opacity = |value| Edit::SetLayerProperty {
        layer: "fill".to_owned(),
        name: "fill-layer-opacity".to_owned(),
        value: serde_json::json!(value),
    };
    let (mut map, drapes) = settled(Tiles::default());
    map.edit(fill_layer_opacity(0.5));
    assert_eq!(drapes, map.frame().1);
    let (full, passes) = map.frame();
    assert!(full);
    assert!(passes.is_empty(), "drapes drawn again: {passes:?}");
    assert!(map.frame().1.is_empty());
    map.edit(fill_layer_opacity(0.7));
    assert_eq!(drapes, map.frame().1);
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
}

/// A value that changes for a reason other than the zoom draws the drapes
/// again even while the camera moves.
#[test]
fn a_global_state_change_draws_every_drape_again_while_the_camera_moves() {
    let (mut map, drapes) = settled(Tiles::default());
    map.camera_moving = true;
    map.edit(Edit::SetGlobalState { name: "opacity".to_owned(), value: feature::Value::Double(0.5) });
    assert_eq!(drapes, map.frame().1);
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
}

/// New data for a source draws again the drapes whose tiles show
/// something else.
#[test]
fn new_data_draws_the_drapes_whose_tiles_changed_again() {
    let (mut map, _) = settled(Tiles::default());
    let geojson: geojson::GeoJson = r#"{
        "type": "Feature",
        "id": 1,
        "properties": {},
        "geometry": {"type": "Polygon", "coordinates": [[[-60, 50], [-50, 50], [-50, 60], [-60, 60], [-60, 50]]]}
    }"#
    .parse()
    .unwrap();
    let options = GeoJSONOptions::default_options();
    let data = mln_style::sources::geojson::create_geojson_data(geojson, &options).unwrap();
    map.edit(GeoJsonSourceType::set_data("shapes", Some(data)));
    // The source indexes its new data on the workers before its tiles lay
    // out again.
    let mut drawn = BTreeSet::new();
    for _ in 0..1000 {
        drawn = map.settle();
        if !drawn.is_empty() {
            break;
        }
    }
    drawn.extend(map.settle());
    // Only the north-west drape's tile shows the polygon.
    assert_eq!(BTreeSet::from([OverscaledTileID::new(2, 0, CanonicalTileID::new(2, 1, 1))]), drawn);
}

#[test]
fn a_new_image_draws_every_drape_again() {
    let (mut map, drapes) = settled(Tiles::default());
    let image = PremultipliedImage::from_vec(Size::new(2, 2), [255, 0, 0, 255].repeat(4));
    let image =
        mln_style::image::Impl::new("red".to_owned(), image, 1.0, false, Vec::new(), Vec::new(), None, None, None)
            .expect("an image");
    map.edit(Edit::AddImage(Arc::new(image)));
    assert_eq!(drapes, map.frame().1);
    assert!(map.frame().1.is_empty());
}

#[test]
fn a_new_source_tile_draws_the_drapes_it_overlaps_again() {
    let held = CanonicalTileID::new(3, 3, 3);
    let tiles = Tiles { held: Mutex::new(Some(("raster/", held))), ..Tiles::default() };
    let (mut map, _) = settled(tiles);
    assert!(!map.tiles.waiting.lock().is_empty(), "the held tile was requested");
    map.tiles.release();
    let drawn = map.settle();
    let overlapping: BTreeSet<OverscaledTileID> =
        map.drapes().into_iter().filter(|drape| held.is_child_of(&drape.canonical)).collect();
    assert_eq!(1, overlapping.len());
    assert_eq!(overlapping, drawn);
}

#[test]
fn a_feature_state_change_draws_its_drapes_again() {
    let (mut map, _) = settled(Tiles::default());
    let mut hover = FeatureState::new();
    hover.insert("hover".into(), feature::Value::Bool(true));
    map.renderer.set_feature_state("shapes", None, "1", &hover);
    // The polygon is in the north-west drape.
    let north_west = OverscaledTileID::new(2, 0, CanonicalTileID::new(2, 1, 1));
    assert_eq!(BTreeSet::from([north_west]), map.frame().1);
    assert!(map.frame().1.is_empty());
}

/// Draws frames until one is complete, and returns the tiles of the drapes
/// of the first stack that each drew. Each frame before it asked for the
/// next.
fn frames_until_full(map: &mut DrapedMap) -> Vec<BTreeSet<OverscaledTileID>> {
    let mut frames = Vec::new();
    for _ in 0..100 {
        let (full, drawn) = map.frame();
        frames.push(drawn);
        if full {
            assert!(!map.needs_repaint, "a complete frame asks for another");
            return frames;
        }
        assert!(map.needs_repaint, "an incomplete frame doesn't ask for another");
    }
    panic!("no frame was complete: {frames:?}");
}

/// Asserts that `frames` drew one tile's drapes each, `drapes` all in
/// turn.
fn assert_one_a_frame(drapes: &BTreeSet<OverscaledTileID>, frames: &[BTreeSet<OverscaledTileID>]) {
    assert_eq!(drapes.len(), frames.len(), "{frames:?}");
    assert!(frames.iter().all(|drawn| drawn.len() == 1), "one drape a frame: {frames:?}");
    assert_eq!(*drapes, frames.iter().flatten().copied().collect::<BTreeSet<_>>());
}

/// A zoom change keeps the drapes while the camera moves. At rest, the
/// drapes draw again one a frame, nearest the center first, and each frame
/// until the last is incomplete and asks for the next.
#[test]
fn a_zoom_change_keeps_the_drapes_while_the_camera_moves_and_draws_them_one_a_frame_at_rest() {
    let (mut map, drapes) = settled(Tiles::default());
    map.camera_moving = true;
    map.transform.jump_to(&CameraOptions { zoom: Some(2.05), ..Default::default() });
    let (full, passes) = map.frame();
    assert!(passes.is_empty(), "drapes drawn while the camera moves: {passes:?}");
    assert!(full, "a moving camera keeps its drapes");
    assert_eq!(drapes, map.drapes(), "the same drapes cover the view");
    map.camera_moving = false;
    let frames = frames_until_full(&mut map);
    assert_one_a_frame(&drapes, &frames);
    let order: Vec<OverscaledTileID> = frames.iter().flatten().copied().collect();
    assert_eq!(map.renderer.drape_tiles(), order, "nearest the center first, as the cover lists them");
    assert!(map.frame().1.is_empty());
}

/// A camera that jumps every frame rests in each: of its drapes stale by
/// the zoom, it draws one a frame, each in turn, and none twice before it
/// drew all, and its frames are incomplete. Once it stops, the drapes it
/// left stale draw one a frame too.
#[test]
fn a_jump_every_frame_draws_one_drape_a_frame_each_in_turn() {
    let (mut map, drapes) = settled(Tiles::default());
    let mut frames = Vec::new();
    for step in 1..=2 * drapes.len() {
        let zoom = 2.0 + 0.01 * step as f64;
        map.transform.jump_to(&CameraOptions { zoom: Some(zoom), ..Default::default() });
        let (full, drawn) = map.frame();
        assert!(!full && map.needs_repaint, "a frame with stale drapes is complete");
        frames.push(drawn);
    }
    assert_eq!(drapes, map.drapes(), "the same drapes cover the view");
    for turn in frames.chunks(drapes.len()) {
        assert_one_a_frame(&drapes, turn);
    }
    let mut stale = drapes.clone();
    stale.remove(frames.last().and_then(|drawn| drawn.first()).unwrap());
    assert_one_a_frame(&stale, &frames_until_full(&mut map));
}

/// A drape whose content changed draws again at once, though the camera
/// left the others stale by the zoom.
#[test]
fn a_paint_change_draws_every_drape_in_one_frame_while_the_camera_jumps() {
    let (mut map, drapes) = settled(Tiles::default());
    map.transform.jump_to(&CameraOptions { zoom: Some(2.05), ..Default::default() });
    assert_eq!(1, map.frame().1.len());
    map.transform.jump_to(&CameraOptions { zoom: Some(2.1), ..Default::default() });
    map.edit(Edit::SetLayerProperty {
        layer: "line".to_owned(),
        name: "line-color".to_owned(),
        value: serde_json::json!("green"),
    });
    let (full, drawn) = map.frame();
    assert_eq!(drapes, drawn);
    assert!(full);
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
}

/// A still draws every drape stale by the zoom in its one frame, though a
/// gesture moves the camera.
#[test]
fn a_still_draws_every_drape_stale_by_the_zoom_at_once() {
    let mut map = DrapedMap::new(Tiles::default());
    map.mode = MapMode::Static;
    map.settle();
    let drapes = map.drapes();
    assert_eq!(4, drapes.len(), "drapes: {drapes:?}");
    for (camera_moving, zoom) in [(false, 2.05), (true, 2.1)] {
        map.camera_moving = camera_moving;
        map.transform.jump_to(&CameraOptions { zoom: Some(zoom), ..Default::default() });
        let (full, drawn) = map.frame();
        assert!(full);
        assert_eq!(drapes, drawn, "camera moving: {camera_moving}");
        assert_eq!(drapes, map.drapes(), "the same drapes cover the view");
    }
}

/// A hillshade of the DEM, lit from the viewport, as its lights are by
/// default.
fn hillshade() -> Edit {
    let json = r#"{
        "version": 8,
        "sources": {"dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 10}},
        "layers": [{"id": "hillshade", "type": "hillshade", "source": "dem"}]
    }"#;
    let parsed = Style::from_json(Registry::with_builtins().style(), json, None).expect("the style");
    let layer = parsed.get_layer_list().iter().next().expect("the layer").clone();
    Edit::AddLayer { layer, before: None }
}

/// A rotation keeps the drapes of layers that draw nothing by the bearing,
/// at rest too. Turned around, the view covers the same drapes.
#[test]
fn a_rotation_keeps_drapes_that_draw_nothing_by_the_bearing() {
    let (mut map, drapes) = settled(Tiles::default());
    map.transform.jump_to(&CameraOptions { bearing: Some(180.0), ..Default::default() });
    let (_, passes) = map.frame();
    assert_eq!(drapes, map.drapes(), "the same drapes cover the view");
    assert!(passes.is_empty(), "drapes drawn again: {passes:?}");
}

/// A hillshade lit from the viewport turns its lights with the map: a
/// rotation keeps its drapes while the camera moves, and draws them again
/// once at rest, one a frame.
#[test]
fn a_rotation_draws_a_viewport_lit_hillshade_again_at_rest() {
    let (mut map, drapes) = settled(Tiles::default());
    map.edit(hillshade());
    map.settle();
    map.camera_moving = true;
    map.transform.jump_to(&CameraOptions { bearing: Some(180.0), ..Default::default() });
    let (_, passes) = map.frame();
    assert_eq!(drapes, map.drapes(), "the same drapes cover the view");
    assert!(passes.is_empty(), "drapes drawn while the camera moves: {passes:?}");
    map.camera_moving = false;
    let frames = frames_until_full(&mut map);
    assert_one_a_frame(&drapes, &frames);
    assert!(map.frame().1.is_empty());
}

/// A DEM whose data expires reloads in place when the file source
/// revalidates it: the hillshade draws every drape that shows it again, and
/// the map stands on the new ground.
#[test]
fn a_reloaded_dem_draws_the_hillshade_drapes_again() {
    let (mut map, drapes) = settled(Tiles::default());
    map.edit(hillshade());
    map.settle();
    let center = map.transform.view().center(WrapMode::Wrapped);
    let height = |map: &DrapedMap| map.elevation.as_ref().and_then(|ground| ground.at(center)).expect("the ground");
    assert!(height(&map).abs() < 1.0, "sea level: {}", height(&map));
    assert!(map.frame().1.is_empty());

    map.tiles.reload_dem_high();
    assert_eq!(drapes, map.settle());
    assert!((height(&map) - 25_000.0).abs() < 1.0, "25 km up: {}", height(&map));
}

/// Draws a frame, and returns its render passes.
fn render_passes(map: &mut DrapedMap) -> i32 {
    map.frame();
    map.renderer.rendering_stats().num_render_passes
}

/// A hillshade on a map without terrain prepares the slopes of each DEM
/// tile once, in a pass of its own: a map at rest draws its frame's pass
/// only, and a DEM tile that arrives prepares its own slopes and those of
/// its three neighbours again, whose borders it fills.
#[test]
fn a_hillshade_prepares_a_dem_tile_once_and_again_when_its_border_arrives() {
    // The north-west one of the four DEM tiles the view sees.
    let held = CanonicalTileID::new(3, 3, 3);
    let mut map = DrapedMap::new(Tiles { held: Mutex::new(Some(("dem/", held))), ..Tiles::default() });
    map.edit(Edit::SetTerrain(None));
    map.edit(hillshade());
    map.settle();
    assert_eq!(1, map.tiles.waiting.lock().len(), "the held tile was requested");
    assert_eq!(1, render_passes(&mut map), "a map at rest prepares nothing");

    map.tiles.release();
    let mut prepared = 0;
    for _ in 0..1000 {
        prepared += render_passes(&mut map) - 1;
        if map.work.idle(0) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    prepared += render_passes(&mut map) - 1;
    assert_eq!(4, prepared, "the new tile and its neighbours prepared");
    assert_eq!(1, render_passes(&mut map), "a map at rest prepares nothing");
}

/// The corners of a box of 10° a side whose north-west corner is at `lat`,
/// `lng`: top left, top right, bottom right, bottom left.
fn corners(lat: f64, lng: f64) -> [LatLng; 4] {
    [(lat, lng), (lat, lng + 10.0), (lat - 10.0, lng + 10.0), (lat - 10.0, lng)]
        .map(|(lat, lng)| LatLng::new(lat, lng).unwrap())
}

/// Adds an image source of a red image with its corners at `corners`, and a
/// raster layer of it above the others.
fn add_image(map: &mut DrapedMap, corners: [LatLng; 4]) {
    map.edit(Edit::AddSource(image_source("image", corners)));
    map.edit(ImageSourceType::set_image(
        "image",
        PremultipliedImage::from_vec(Size::new(2, 2), [255, 0, 0, 255].repeat(4)),
    ));
    let json = r#"{
        "version": 8,
        "sources": {"image": {"type": "image", "coordinates": [[0, 0], [1, 0], [1, -1], [0, -1]]}},
        "layers": [{"id": "image", "type": "raster", "source": "image", "paint": {"raster-fade-duration": 0}}]
    }"#;
    let parsed = Style::from_json(Registry::with_builtins().style(), json, None).expect("the style");
    let layer = parsed.get_layer_list().iter().next().expect("the layer").clone();
    map.edit(Edit::AddLayer { layer, before: None });
}

/// Whether `pixel` is the image's red, and not the grey and white under it.
fn red(pixel: [u8; 4]) -> bool {
    let [r, g, b, _] = pixel;
    r > 200 && g < 50 && b < 50
}

/// An image source drapes: moving its image draws again the drapes that
/// showed it and those that show it now, and no others.
#[test]
fn moving_an_image_draws_the_drapes_it_left_and_entered_again() {
    let (mut map, drapes) = settled(Tiles::default());
    // In the south-east drape's tile.
    add_image(&mut map, corners(-20.0, 20.0));
    // A new layer draws every drape again.
    assert_eq!(drapes, map.settle());
    assert!(map.frame().1.is_empty());
    // Into the south-west drape's tile.
    map.edit(ImageSourceType::set_coordinates("image", corners(-20.0, -30.0)));
    let south_west = OverscaledTileID::new(2, 0, CanonicalTileID::new(2, 1, 2));
    let south_east = OverscaledTileID::new(2, 0, CanonicalTileID::new(2, 2, 2));
    assert_eq!(BTreeSet::from([south_west, south_east]), map.settle());
    assert_eq!(drapes, map.drapes());
}

/// An image shrunk to under two pixels draws no more, and the drape that
/// showed it draws again without it.
#[test]
fn an_image_shrunk_out_of_sight_draws_its_drape_again() {
    let (mut map, _) = settled(Tiles::default());
    add_image(&mut map, corners(-20.0, 20.0));
    map.settle();
    // 0.1° is half a pixel at zoom 2.
    let tiny =
        [(-20.0, 20.0), (-20.0, 20.1), (-20.1, 20.1), (-20.1, 20.0)].map(|(lat, lng)| LatLng::new(lat, lng).unwrap());
    map.edit(ImageSourceType::set_coordinates("image", tiny));
    let south_east = OverscaledTileID::new(2, 0, CanonicalTileID::new(2, 2, 2));
    assert_eq!(BTreeSet::from([south_east]), map.frame().1);
    assert!(map.frame().1.is_empty(), "the drape is drawn again once only");
}

/// An image across the antimeridian draws its part east of it in the world
/// copy west of the image's own: the view east of the antimeridian shows it.
#[test]
fn an_image_across_the_antimeridian_draws_where_the_view_sees_it() {
    let mut map = DrapedMap::new(Tiles::default());
    map.transform.jump_to(&CameraOptions {
        center: Some(LatLng::new(-10.0, -176.0).unwrap()),
        zoom: Some(6.0),
        ..Default::default()
    });
    map.settle();
    // From 170° to 186°, that is to -174°.
    let corners = [(-5.0, 170.0), (-5.0, 186.0), (-15.0, 186.0), (-15.0, 170.0)];
    add_image(&mut map, corners.map(|(lat, lng)| LatLng::new(lat, lng).unwrap()));
    map.settle();
    let (full, _) = map.frame();
    assert!(full);
    let center = SIDE / 2;
    let pixel = map.pixel(center, center);
    assert!(red(pixel), "the image at the view's center: {pixel:?}");
}

/// The longitude `phase` of the way across a tile of zoom 14 east of 10°.
fn lng_in_tile(phase: f64) -> f64 {
    let tiles = f64::from(1u32 << 14);
    let x = ((10.0 + 180.0) / 360.0 * tiles).floor();
    (x + phase) / tiles * 360.0 - 180.0
}

/// A square image 0.012° a side, a little over half a tile of zoom 14,
/// centered at latitude -10° and `lng`.
fn small_square(lng: f64) -> [LatLng; 4] {
    let half = 0.006;
    [(-10.0 + half, lng - half), (-10.0 + half, lng + half), (-10.0 - half, lng + half), (-10.0 - half, lng - half)]
        .map(|(lat, lng)| LatLng::new(lat, lng).unwrap())
}

/// A map over ground 25 km high at zoom 12, centered at latitude -10° and
/// `lng`, pitched by `pitch`, settled with the camera on the ground.
fn raised(lng: f64, pitch: f64) -> DrapedMap {
    let mut map = DrapedMap::new(Tiles { high: true, ..Tiles::default() });
    map.transform.jump_to(&CameraOptions {
        center: Some(LatLng::new(-10.0, lng).unwrap()),
        zoom: Some(12.0),
        pitch: Some(pitch),
        ..Default::default()
    });
    map.settle_on_ground();
    let altitude = map.transform.view().center_altitude();
    assert!((altitude - 25_000.0).abs() < 1.0, "the camera stands on the ground: {altitude}");
    map
}

/// Pitched over high ground, the view's center is far nearer the camera
/// than any ground at sea level in view: an image there loads and draws.
#[test]
fn an_image_on_raised_ground_loads_and_draws() {
    let mut map = raised(10.0, 60.0);
    add_image(&mut map, small_square(10.0));
    map.settle();
    let (full, _) = map.frame();
    assert!(full, "the image loads");
    let center = SIDE / 2;
    let pixel = map.pixel(center, center);
    assert!(red(pixel), "the image at the view's center: {pixel:?}");
}

/// An image that moves within its tile to ground that only its height
/// brings into view draws where it moved to.
#[test]
fn an_image_moved_onto_raised_ground_draws_there() {
    let (west, east) = (lng_in_tile(0.06), lng_in_tile(0.94));
    // Seen from above, sea level covers more of the world than the ground
    // in view: the image loads.
    let mut map = raised(east, 0.0);
    add_image(&mut map, small_square(west));
    map.settle();
    assert!(map.frame().0, "the image loads");
    map.transform.jump_to(&CameraOptions { pitch: Some(60.0), ..Default::default() });
    map.settle_on_ground();
    map.edit(ImageSourceType::set_coordinates("image", small_square(east)));
    map.settle();
    let (full, _) = map.frame();
    assert!(full);
    let center = SIDE / 2;
    let pixel = map.pixel(center, center);
    assert!(red(pixel), "the image at the view's center: {pixel:?}");
}

/// A red circle 8 px across at `location`.
fn red_circle(location: LatLng) -> String {
    format!(
        r#"{{
            "version": 8,
            "sources": {{
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [{}, {}]}}}}
            }},
            "layers": [{{"id": "circle", "type": "circle", "source": "point", "paint": {{"circle-color": "red", "circle-radius": 4}}}}]
        }}"#,
        location.longitude(),
        location.latitude()
    )
}

impl DrapedMap {
    /// The pixel where the last frame's camera sees `location` `height`
    /// meters above sea level.
    fn pixel_at(&mut self, location: &LatLng, height: f64) -> [u8; 4] {
        let point = self.transform.view().lat_lng_to_screen_coordinate_at(location, height);
        let (x, y) = (point.x.round() as u32, (f64::from(SIDE) - point.y).round() as u32);
        assert!(x < SIDE && y < SIDE, "{location:?} at {height} m is off the screen at {x}, {y}");
        self.pixel(x, y)
    }
}

/// A circle stands on the ground: over ground 25 km high, it draws where the
/// camera sees its center 25 km up, not at sea level. Once the terrain
/// turns off, the next frame draws it at sea level, and once it turns back
/// on, the next one on the ground again.
#[test]
fn toggling_terrain_moves_a_circle_in_the_next_frame() {
    let mut map = raised(10.0, 0.0);
    // About 50 px east of the view's center on the ground, and some 11 px
    // at sea level, which is farther from the camera.
    let location = LatLng::new(-10.0, 10.0086).unwrap();
    add_layer(&mut map, &red_circle(location), None);
    map.settle();
    map.frame();
    assert!(red(map.pixel_at(&location, 25_000.0)), "the circle on the ground");
    assert!(!red(map.pixel_at(&location, 0.0)), "no circle at sea level");

    let terrain = map.style.get_terrain().map(|terrain| (**terrain).clone());
    map.edit(Edit::SetTerrain(None));
    map.frame();
    assert!(red(map.pixel_at(&location, 0.0)), "the circle at sea level");
    assert!(!red(map.pixel_at(&location, 25_000.0)), "no circle on the ground that went");

    map.edit(Edit::SetTerrain(terrain));
    map.frame();
    assert!(red(map.pixel_at(&location, 25_000.0)), "the circle on the ground again");
    assert!(!red(map.pixel_at(&location, 0.0)), "no circle at sea level");
}

/// A map without terrain draws its circles flat, with no terrain variant
/// and no ground: no DEM texture.
#[test]
fn a_map_without_terrain_makes_no_terrain_variant() {
    let mut map = DrapedMap::new(Tiles::default());
    map.edit(Edit::SetTerrain(None));
    add_layer(&mut map, &red_circle(LatLng::new(-10.0, 10.0).unwrap()), None);
    map.settle();
    assert!(map.frame().0);
    let center = SIDE / 2;
    assert!(red(map.pixel(center, center)), "the circle");
    assert!(!map.renderer.made_terrain_variant());
    assert_eq!(0, map.renderer.rendering_stats().terrain.mem_dem_textures);
}

/// A terrain map whose layers all drape holds DEM textures of the DEM tiles
/// its surface draws with, and none of the coarse ones it holds to cull the
/// surface's tiles: only the standing layers' parts off the cover read
/// those, once a circle stands on the ground.
#[test]
fn a_map_without_a_standing_layer_makes_no_texture_of_a_dem_tile_its_surface_doesnt_read() {
    let mut map = raised(10.0, 0.0);
    map.settle();
    map.frame();
    let (textures, read) = map.renderer.dem_textures();
    assert!(read > 0);
    assert_eq!(read, textures, "the DEM tiles the surface reads");

    add_layer(&mut map, &red_circle(LatLng::new(-10.0, 10.0086).unwrap()), None);
    map.settle();
    map.frame();
    let (textures, read) = map.renderer.dem_textures();
    assert!(textures > read, "{textures} DEM textures, of which {read} the surface reads, and the coarse ones");
}

/// Once terrain turns off, a circle that stood on it draws with the flat
/// variant, whose draws are made again without the ground, and the DEM
/// textures go.
#[test]
fn a_map_whose_terrain_turned_off_keeps_no_ground() {
    let mut map = raised(10.0, 0.0);
    add_layer(&mut map, &red_circle(LatLng::new(-10.0, 10.0086).unwrap()), None);
    map.settle();
    map.frame();
    let circle = |map: &DrapedMap| {
        let variants = map.renderer.projection_variants();
        variants.into_iter().find_map(|(id, variant)| (id == "circle").then_some(variant))
    };
    assert_eq!(Some(ProjectionVariant::Terrain), circle(&map));
    assert_ne!(0, map.renderer.rendering_stats().terrain.mem_dem_textures);

    map.edit(Edit::SetTerrain(None));
    map.frame();
    assert_eq!(Some(ProjectionVariant::Mercator), circle(&map));
    assert_eq!(0, map.renderer.rendering_stats().terrain.mem_dem_textures);
}

/// A red fill extrusion `height` meters tall, a square `half` degrees from
/// `location` to each side.
fn red_extrusion(location: LatLng, half: f64, height: f64) -> String {
    let (lat, lng) = (location.latitude(), location.longitude());
    format!(
        r#"{{
            "version": 8,
            "sources": {{
                "block": {{"type": "geojson", "data": {{"type": "Polygon", "coordinates": [[
                    [{west}, {south}], [{east}, {south}], [{east}, {north}], [{west}, {north}], [{west}, {south}]
                ]]}}}}
            }},
            "layers": [{{
                "id": "extrusion",
                "type": "fill-extrusion",
                "source": "block",
                "paint": {{"fill-extrusion-color": "red", "fill-extrusion-height": {height}}}
            }}]
        }}"#,
        west = lng - half,
        east = lng + half,
        south = lat - half,
        north = lat + half,
    )
}

/// A fill extrusion stands on the ground: over ground 25 km high, the camera
/// sees its roof 25 km and its height up, not its height above sea level.
/// Once the terrain turns off, the next frame draws it on the sea, and once
/// it turns back on, the next one on the ground again.
#[test]
fn toggling_terrain_moves_an_extrusion_in_the_next_frame() {
    let mut map = raised(10.0, 0.0);
    // About 50 px east of the view's center on the ground, and some 11 px
    // at sea level, which is farther from the camera.
    let location = LatLng::new(-10.0, 10.0086).unwrap();
    add_layer(&mut map, &red_extrusion(location, 0.002, 500.0), None);
    map.settle();
    map.frame();
    assert!(red(map.pixel_at(&location, 25_500.0)), "the roof on the ground");
    assert!(!red(map.pixel_at(&location, 500.0)), "no roof above sea level");

    let terrain = map.style.get_terrain().map(|terrain| (**terrain).clone());
    map.edit(Edit::SetTerrain(None));
    map.frame();
    assert!(red(map.pixel_at(&location, 500.0)), "the roof above sea level");
    assert!(!red(map.pixel_at(&location, 25_500.0)), "no roof on the ground that went");

    map.edit(Edit::SetTerrain(terrain));
    map.frame();
    assert!(red(map.pixel_at(&location, 25_500.0)), "the roof on the ground again");
    assert!(!red(map.pixel_at(&location, 500.0)), "no roof above sea level");
}

/// A map without terrain draws its fill extrusions flat, with draws that
/// keep no centroid, per vertex or in a storage buffer. Over terrain they
/// keep them.
#[test]
fn a_map_without_terrain_keeps_no_extrusion_centroids() {
    use mln_gfx::shaders::shader_defines::{ID_FILL_EXTRUSION_CENTROID_ATTRIBUTE, ID_FILL_EXTRUSION_WALL_CENTROIDS};

    let mut map = DrapedMap::new(Tiles::default());
    let terrain = map.style.get_terrain().map(|terrain| (**terrain).clone());
    map.edit(Edit::SetTerrain(None));
    add_layer(&mut map, &red_extrusion(LatLng::new(-10.0, 10.0).unwrap(), 2.0, 500.0), None);
    map.settle();
    assert!(map.frame().0);
    let center = SIDE / 2;
    assert!(red(map.pixel(center, center)), "the extrusion");
    let keeps = |map: &mut DrapedMap, id| map.renderer.keeps_vertices_of("extrusion", id);
    assert!(!keeps(&mut map, ID_FILL_EXTRUSION_CENTROID_ATTRIBUTE));
    assert!(!keeps(&mut map, ID_FILL_EXTRUSION_WALL_CENTROIDS));
    assert!(!map.renderer.made_terrain_variant());

    map.edit(Edit::SetTerrain(terrain));
    map.settle();
    map.frame();
    assert!(keeps(&mut map, ID_FILL_EXTRUSION_CENTROID_ATTRIBUTE), "the roofs' centroids");
}
