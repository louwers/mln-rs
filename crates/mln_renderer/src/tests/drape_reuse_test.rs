// A continuous map over terrain draws a drape again only when what it
// shows changed: its layers' values, the tiles they draw, their feature
// state, or, once the camera rests, the zoom.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use mln_gfx::RendererBackend as _;
use mln_gfx::frame::FrameTarget;
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_map::transform::Transform;
use mln_map_types::{
    CameraOptions, ConstrainMode, MapDebugOptions, MapMode, TerrainSkirtLength, TileLodMode, ViewportMode,
};
use mln_storage::{FileSource, Resource, Responder, Response};
use mln_style::expression::expression::FeatureState;
use mln_style::sources::{GeoJSONOptions, GeoJsonSourceType};
use mln_style::{Edit, Style};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::geo::LatLng;
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

/// Answers every tile request at once: sea level for the DEM, grey for the
/// raster source, except a raster tile that waits while it is held.
#[derive(Default)]
struct Tiles {
    held: Mutex<Option<CanonicalTileID>>,
    waiting: Mutex<Vec<Responder>>,
}

impl FileSource for Tiles {
    fn load(&self, resource: &Resource, responder: Responder) {
        let data = resource.tile_data.as_ref().expect("only tiles are requested");
        let tile = CanonicalTileID::new(data.z as u8, data.x as u32, data.y as u32);
        if data.url_template.starts_with("dem/") {
            // (1 · 65536 + 134 · 256 + 160) · 0.1 − 10000 = 0 m.
            responder.send(png(256, [1, 134, 160, 255]));
        } else if *self.held.lock() == Some(tile) {
            self.waiting.lock().push(responder);
        } else {
            responder.send(png(256, [128, 128, 128, 255]));
        }
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

impl Tiles {
    /// Answers the held tile.
    fn release(&self) {
        *self.held.lock() = None;
        for responder in std::mem::take(&mut *self.waiting.lock()) {
            responder.send(png(256, [128, 128, 128, 255]));
        }
    }
}

/// [`STYLE`], with what its sources' types know without a fetch.
fn style() -> Style {
    let registry = Registry::with_builtins();
    let parsed = Style::from_json(registry.style(), STYLE, None).expect("the style");
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

/// A continuous map of [`style`] over WebGPU, drawn frame by frame.
struct DrapedMap {
    backend: mln_webgpu::RendererBackend,
    target: OffscreenTarget<mln_webgpu::WebGpu>,
    renderer: Renderer,
    events: RendererEvents,
    tiles: Arc<Tiles>,
    style: Arc<Style>,
    transform: Transform,
    camera_moving: bool,
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
            backend,
            target: OffscreenTarget::new(Size::new(SIDE, SIDE)),
            renderer,
            events,
            tiles: Arc::new(tiles),
            style: Arc::new(style()),
            transform,
            camera_moving: false,
            work: TileWork::default(),
            _pool: pool,
        }
    }

    fn update_parameters(&self) -> UpdateParameters {
        let file_source: Arc<dyn FileSource> = Arc::<Tiles>::clone(&self.tiles);
        UpdateParameters {
            style_loaded: true,
            mode: MapMode::Continuous,
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
            still_image_request: false,
            still_image_request_id: 0,
            cross_source_collisions: true,
            fast_pfor_enabled: false,
            terrain_skirt_length: TerrainSkirtLength::Auto,
            ground_generation: 0,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: mln_math::deg2rad(60.0),
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
        }
    }

    /// Draws a frame, and returns whether everything it shows had loaded and
    /// the tiles of the drapes it drew.
    fn frame(&mut self) -> (bool, BTreeSet<OverscaledTileID>) {
        let parameters = Arc::new(self.update_parameters());
        let gpu = self.backend.gpu();
        let frame = self.renderer.update(gpu, &parameters).expect("a continuous map's frame");
        let image = self.target.begin_frame(gpu).expect("the frame's image");
        let drawn = self.renderer.render(gpu, &image, frame);
        let handoff = self.target.end_frame(gpu);
        self.renderer.finish_frame(gpu, drawn, handoff);
        let mut full = false;
        for event in self.events.take() {
            match event {
                RendererEvent::RenderFrameFinished { mode, .. } => full = mode == RenderMode::Full,
                RendererEvent::TileAction { op, id, source_id } => self.work.record(op, (source_id, id)),
                _ => {}
            }
        }
        let passes = self.renderer.take_drape_passes();
        let unique: BTreeSet<OverscaledTileID> = passes.iter().copied().collect();
        assert_eq!(unique.len(), passes.len(), "a drape drawn twice in a frame");
        (full, unique)
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
    let tiles = Tiles { held: Mutex::new(Some(held)), ..Tiles::default() };
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

#[test]
fn a_zoom_change_keeps_the_drapes_while_the_camera_moves_and_draws_them_at_rest() {
    let (mut map, drapes) = settled(Tiles::default());
    map.camera_moving = true;
    map.transform.jump_to(&CameraOptions { zoom: Some(2.05), ..Default::default() });
    let (_, passes) = map.frame();
    assert!(passes.is_empty(), "drapes drawn while the camera moves: {passes:?}");
    assert_eq!(drapes, map.drapes(), "the same drapes cover the view");
    map.camera_moving = false;
    assert_eq!(drapes, map.frame().1, "the drapes drawn again at rest");
    assert!(map.frame().1.is_empty());
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
/// once at rest.
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
    assert_eq!(drapes, map.frame().1, "the drapes drawn again at rest");
    assert!(map.frame().1.is_empty());
}
