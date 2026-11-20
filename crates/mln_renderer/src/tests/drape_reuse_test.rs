// A continuous map over terrain draws a drape again only when what it
// shows changed: its layers' values, the tiles or image they draw, their
// feature state, or, once the camera rests, the zoom.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use mln_gfx::RendererBackend as _;
use mln_gfx::frame::FrameTarget;
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_map::transform::Transform;
use mln_map_types::{
    CameraOptions, ConstrainMode, Elevation, MapDebugOptions, MapMode, TerrainSkirtLength, TileLodMode, ViewportMode,
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
/// `high`, and grey for the raster source, except a raster tile that waits
/// while it is held.
#[derive(Default)]
struct Tiles {
    high: bool,
    held: Mutex<Option<CanonicalTileID>>,
    waiting: Mutex<Vec<Responder>>,
}

impl FileSource for Tiles {
    fn load(&self, resource: &Resource, responder: Responder) {
        let data = resource.tile_data.as_ref().expect("only tiles are requested");
        let tile = CanonicalTileID::new(data.z as u8, data.x as u32, data.y as u32);
        if data.url_template.starts_with("dem/") {
            // (1 · 65536 + 134 · 256 + 160) · 0.1 − 10000 = 0 m, and
            // (5 · 65536 + 87 · 256 + 48) · 0.1 − 10000 = 25000 m.
            responder.send(png(256, if self.high { [5, 87, 48, 255] } else { [1, 134, 160, 255] }));
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
    /// The ground the renderer last told of, which the updates carry, as a
    /// map's do.
    elevation: Option<Arc<dyn Elevation>>,
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
            elevation: None,
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
    /// the tiles and stacks of the drapes it drew.
    fn frame_passes(&mut self) -> (bool, BTreeSet<(OverscaledTileID, usize)>) {
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
                RendererEvent::TerrainElevation(elevation) => self.elevation = elevation,
                RendererEvent::TileAction { op, id, source_id } => self.work.record(op, (source_id, id)),
                _ => {}
            }
        }
        let passes = self.renderer.take_drape_passes();
        let unique: BTreeSet<(OverscaledTileID, usize)> = passes.iter().copied().collect();
        assert_eq!(unique.len(), passes.len(), "a drape drawn twice in a frame");
        (full, unique)
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
            let center = self.transform.view().center(WrapMode::Wrapped);
            let (altitude, min) = (elevation.at(center), elevation.min_at(center).unwrap_or(0.0));
            if !self.transform.set_ground(altitude, Some(min)) {
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

/// [`STYLE`] with a circle under its line, which splits its layers into two
/// stacks, settled, and its four drapes.
fn two_stacks() -> (DrapedMap, BTreeSet<OverscaledTileID>) {
    let (mut map, drapes) = settled(Tiles::default());
    add_layer(&mut map, CIRCLE, Some("line"));
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

/// Without a draped layer, a map over terrain has no drape and draws no
/// surface: only its circle.
#[test]
fn a_map_without_a_draped_layer_draws_no_surface() {
    let (mut map, _) = settled(Tiles::default());
    for layer in ["background", "raster", "fill", "line"] {
        map.edit(Edit::RemoveLayer(layer.to_owned()));
    }
    add_layer(&mut map, CIRCLE, None);
    let drawn = map.settle();
    assert!(drawn.is_empty(), "drapes drawn: {drawn:?}");
    let (full, passes) = map.frame_passes();
    assert!(full);
    assert!(passes.is_empty(), "drapes drawn: {passes:?}");
    assert_eq!(4, map.drapes().len(), "terrain still covers the view");
    assert_eq!(1, map.draw_calls());
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
