// A map over terrain draws a drape again only when what it shows changed:
// its layers or their values, the tiles or image they draw, their feature
// state, the style's images, the zoom, or the bearing for a layer that draws
// by it, but not for a hillshade lit from the viewport, which the surface
// shades. A byte cap halves the drapes of the tiles farthest from the center
// first.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use mln_gfx::RendererBackend as _;
use mln_gfx::frame::FrameTarget;
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_map::transform::Transform;
use mln_map_types::{CameraOptions, ConstrainMode, Elevation, MapMode, TerrainOptions, TileLodMode, ViewportMode};
use mln_storage::{FileSource, Resource, Responder, Response};
use mln_style::bitmap::PremultipliedImage;
use mln_style::feature::FeatureState;
use mln_style::geo::LatLng;
use mln_style::size::Size;
use mln_style::sources::{GeoJSONOptions, GeoJsonSourceType};
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use mln_style::{Edit, Style};
use mln_tile_id::OverscaledTileID;
use parking_lot::Mutex;

use crate::Registry;
use crate::render::events::{RenderMode, RendererEvent, RendererEvents};
use crate::render::renderer::Renderer;
use crate::render::update_parameters::UpdateParameters;
use crate::tile_store::tile::TileOperation;

const SIDE: u32 = 256;

/// A white map with a raster layer, a fill whose color follows the feature
/// state "hover", and a line, over a terrain of flat ground.
const STYLE: &str = r#"{
    "version": 8,
    "transition": {"duration": 0},
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
            "paint": {"fill-color": ["case", ["boolean", ["feature-state", "hover"], false], "red", "blue"]}
        },
        {"id": "line", "type": "line", "source": "shapes", "paint": {"line-width": 4}}
    ]
}"#;

/// A PNG of 256 pixels a side, every pixel `pixel`.
fn png(pixel: [u8; 4]) -> Response {
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for texel in image.data.as_chunks_mut::<4>().0 {
        *texel = pixel;
    }
    let mut response = Response::new();
    response.data = Some(Arc::new(mln_image::encode_png(&image)));
    response
}

/// Answers every tile request at once, sea level for the DEM, grey for the
/// raster source, but the raster tile `held`, which waits until released.
#[derive(Default)]
struct Tiles {
    held: Mutex<Option<CanonicalTileID>>,
    waiting: Mutex<Vec<(Responder, Response)>>,
}

impl FileSource for Tiles {
    fn load(&self, resource: &Resource, responder: Responder) {
        let data = resource.tile_data.as_ref().expect("only tiles are requested");
        let tile = CanonicalTileID::new(data.z as u8, data.x as u32, data.y as u32);
        let dem = data.url_template.starts_with("dem/");
        // (1 · 65536 + 134 · 256 + 160) · 0.1 − 10000 = 0 m.
        let response = png(if dem { [1, 134, 160, 255] } else { [128, 128, 128, 255] });
        if !dem && *self.held.lock() == Some(tile) {
            self.waiting.lock().push((responder, response));
        } else {
            responder.send(response);
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
        for (responder, response) in std::mem::take(&mut *self.waiting.lock()) {
            responder.send(response);
        }
    }
}

/// The style of `json`, with what its sources' types know without a fetch.
fn style_of(json: &str) -> Style {
    let registry = Registry::with_builtins();
    let parsed = Style::from_json(registry.style(), json).expect("the style");
    let mut style = Style::new(Arc::clone(registry.style()));
    for source in parsed.get_sources().iter() {
        style.apply(Edit::AddSource(crate::tests::ready(source.clone()))).unwrap();
    }
    for layer in parsed.get_layer_list().iter() {
        style.apply(Edit::AddLayer { layer: layer.clone(), before: None }).unwrap();
    }
    style.apply(Edit::SetTerrain(parsed.get_terrain().map(|terrain| (**terrain).clone()))).unwrap();
    style.apply(Edit::SetTransition(parsed.get_transition_options())).unwrap();
    style
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

/// A continuous map of [`STYLE`] over WebGPU, at zoom 2 over 10° E, 10° S,
/// seen from above over four terrain tiles, drawn frame by frame.
struct DrapedMap {
    backend: mln_webgpu::RendererBackend,
    target: OffscreenTarget<mln_webgpu::WebGpu>,
    renderer: Renderer,
    events: RendererEvents,
    tiles: Arc<Tiles>,
    style: Arc<Style>,
    transform: Transform,
    /// The ground the renderer last told of, which the updates carry, as a
    /// map's do.
    elevation: Option<Arc<dyn Elevation>>,
    /// The terrain options the updates carry.
    terrain: TerrainOptions,
    work: TileWork,
    _pool: mln_util::pool::Pool,
}

impl DrapedMap {
    fn new(tiles: Tiles) -> Option<Self> {
        let device = super::webgpu_device()?;
        let mut backend = mln_webgpu::RendererBackend::new(&device);
        let pool = crate::tests::pool();
        let mut renderer =
            Renderer::new(backend.gpu(), 1.0, mln_text::local_fonts::LocalGlyphs::default(), Arc::new(|| {}), &pool);
        let events = RendererEvents::new();
        renderer.set_event_queue(Some(events.clone()));
        let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(Size::new(SIDE, SIDE)).unwrap();
        transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(-10.0, 10.0).unwrap()),
            zoom: Some(2.0),
            ..Default::default()
        });
        Some(DrapedMap {
            backend,
            target: OffscreenTarget::new(mln_gfx::Size::new(SIDE, SIDE)),
            renderer,
            events,
            tiles: Arc::new(tiles),
            style: Arc::new(style_of(STYLE)),
            transform,
            elevation: None,
            terrain: TerrainOptions::default(),
            work: TileWork::default(),
            _pool: pool,
        })
    }

    fn update_parameters(&self) -> UpdateParameters {
        let file_source: Arc<dyn FileSource> = Arc::<Tiles>::clone(&self.tiles);
        UpdateParameters {
            style_loaded: true,
            mode: MapMode::Continuous,
            pixel_ratio: 1.0,
            time_point: None,
            view: self.transform.view().clone(),
            min_zoom: 0.0,
            max_zoom: 22.0,
            camera_moving: false,
            style: Arc::clone(&self.style),
            font_faces: None,
            sprite_loaded: true,
            feature_states: Arc::default(),
            file_source: Some(file_source),
            prefetch_zoom_delta: 0,
            still_image_request: false,
            still_image_request_id: 0,
            update_id: 0,
            cross_source_collisions: true,
            fast_pfor_enabled: false,
            terrain: self.terrain,
            elevation: self.elevation.clone(),
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: mln_style::math::deg2rad(60.0),
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
        }
    }

    /// Draws a frame, and returns whether everything it shows had loaded and
    /// the tiles and stacks of the drapes it drew.
    fn frame(&mut self) -> (bool, BTreeSet<(OverscaledTileID, usize)>) {
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
                RendererEvent::RenderFrameFinished { mode, error, .. } => {
                    assert!(error.is_none(), "the frame failed: {error:?}");
                    full = mode == RenderMode::Full;
                }
                RendererEvent::TerrainElevation(elevation) => self.elevation = elevation,
                RendererEvent::TileAction { op, id, source_id } => self.work.record(op, (source_id, id)),
                _ => {}
            }
        }
        let drawn = self.renderer.terrain().drapes().drawn();
        assert_eq!(drawn.len(), self.renderer.rendering_stats().terrain.drapes_drawn);
        (full, drawn.into_iter().collect())
    }

    /// Draws frames until the renderer waits for nothing but the held tile,
    /// and then one more. Returns the tiles and stacks of the drapes they
    /// drew.
    fn settle(&mut self) -> BTreeSet<(OverscaledTileID, usize)> {
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

    /// The last frame's pixels.
    fn image(&mut self) -> Vec<u8> {
        self.target.read_image(self.backend.gpu()).expect("the frame's pixels").data.clone()
    }

    /// The tiles and stacks of the drapes that have a texture.
    fn textured(&mut self) -> BTreeSet<(OverscaledTileID, usize)> {
        self.renderer.terrain().drapes().textured().into_iter().collect()
    }

    /// The sides of the drapes, by tile, in the cover's order.
    fn sides(&mut self) -> Vec<(OverscaledTileID, u32)> {
        self.renderer.terrain().drapes().sizes()
    }

    fn edit(&mut self, edit: Edit) {
        let mut style = (*self.style).clone();
        style.apply(edit).unwrap();
        self.style = Arc::new(style);
    }

    /// Adds the first layer of `json`, a style's, with its sources, before
    /// the layer `before` or on top.
    fn add_layer(&mut self, json: &str, before: Option<&str>) {
        let parsed = Style::from_json(Registry::with_builtins().style(), json).expect("the style");
        for source in parsed.get_sources().iter() {
            self.edit(Edit::AddSource(crate::tests::ready(source.clone())));
        }
        let layer = parsed.get_layer_list().iter().next().expect("the layer").clone();
        self.edit(Edit::AddLayer { layer, before: before.map(str::to_owned) });
    }
}

impl Drop for DrapedMap {
    fn drop(&mut self) {
        let gpu = self.backend.gpu();
        self.target.tear_down(gpu);
    }
}

/// The map, settled, and the tiles and stacks of its drapes, one stack over
/// four tiles.
fn settled(tiles: Tiles) -> Option<(DrapedMap, BTreeSet<(OverscaledTileID, usize)>)> {
    let mut map = DrapedMap::new(tiles)?;
    map.settle();
    let drapes = map.textured();
    assert_eq!(4, drapes.len(), "drapes: {drapes:?}");
    Some((map, drapes))
}

/// The terrain tile at zoom 2 north-west of the view's center, which holds
/// the shapes.
fn north_west() -> OverscaledTileID {
    OverscaledTileID::new(2, 0, CanonicalTileID::new(2, 1, 1))
}

#[test]
fn a_map_at_rest_draws_no_drape_again() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
    let (full, drawn) = map.frame();
    assert!(full);
    assert!(drawn.is_empty(), "drapes drawn again: {drawn:?}");
    assert_eq!(drapes, map.textured());
}

/// A change of the zoom changes what a drape shows, line widths and the
/// layers' values, so every drape draws again, once.
#[test]
fn a_zoom_change_draws_every_drape_again() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
    map.transform.jump_to(&CameraOptions { zoom: Some(2.1), ..Default::default() });
    let (_, drawn) = map.frame();
    assert_eq!(drapes, map.textured(), "the same drapes cover the view");
    assert_eq!(drapes, drawn);
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
}

/// A pan that keeps the view over the same terrain tiles keeps their
/// drapes.
#[test]
fn a_pan_keeps_the_drapes() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
    map.transform.jump_to(&CameraOptions { center: Some(LatLng::new(-9.0, 11.0).unwrap()), ..Default::default() });
    let drawn = map.settle();
    assert_eq!(drapes, map.textured(), "the same drapes cover the view");
    assert!(drawn.is_empty(), "drapes drawn again: {drawn:?}");
}

/// A circle at one point, in the north-west drape's tile.
const CIRCLE: &str = r#"{
    "version": 8,
    "sources": {
        "point": {"type": "geojson", "data": {"type": "Point", "coordinates": [-75, 55]}}
    },
    "layers": [{"id": "circle", "type": "circle", "source": "point"}]
}"#;

/// A change of a layer in the second of two stacks draws only that stack's
/// drapes again.
#[test]
fn a_paint_change_in_a_stack_draws_only_its_drapes_again() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
    map.add_layer(CIRCLE, Some("raster"));
    map.settle();
    let both: BTreeSet<_> = drapes.iter().flat_map(|&(tile, _)| [(tile, 0), (tile, 1)]).collect();
    assert_eq!(both, map.textured(), "two stacks that both draw in every tile");
    assert!(map.frame().1.is_empty());
    map.edit(Edit::SetLayerProperty {
        layer: "raster".to_owned(),
        name: "raster-opacity".to_owned(),
        value: serde_json::json!(0.7),
    });
    let second: BTreeSet<_> = drapes.iter().map(|&(tile, _)| (tile, 1)).collect();
    assert_eq!(second, map.frame().1);
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");
}

/// New data for a source draws again the drapes whose tiles show something
/// else.
#[test]
fn new_data_draws_the_drapes_whose_tiles_changed_again() {
    let Some((mut map, _)) = settled(Tiles::default()) else {
        return;
    };
    let geojson: geojson::GeoJson = r#"{
        "type": "Feature",
        "id": 1,
        "properties": {},
        "geometry": {"type": "Polygon", "coordinates": [[[-60, 50], [-50, 50], [-50, 60], [-60, 60], [-60, 50]]]}
    }"#
    .parse()
    .unwrap();
    let data = mln_style::sources::geojson::create_geojson_data(geojson, &GeoJSONOptions::default_options()).unwrap();
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
    assert_eq!(BTreeSet::from([(north_west(), 0)]), drawn, "only the north-west tile shows the polygon");
}

/// The drapes' keys don't tell what the style's images draw.
#[test]
fn a_new_image_draws_every_drape_again() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
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
    let Some((mut map, drapes)) = settled(Tiles { held: Mutex::new(Some(held)), ..Tiles::default() }) else {
        return;
    };
    assert!(!map.tiles.waiting.lock().is_empty(), "the held tile was requested");
    map.tiles.release();
    let drawn = map.settle();
    let overlapping: BTreeSet<_> = drapes.into_iter().filter(|(drape, _)| held.is_child_of(&drape.canonical)).collect();
    assert_eq!(1, overlapping.len());
    assert_eq!(overlapping, drawn);
}

#[test]
fn a_feature_state_change_draws_its_drapes_again() {
    let Some((mut map, _)) = settled(Tiles::default()) else {
        return;
    };
    let mut hover = FeatureState::new();
    hover.insert("hover".into(), feature::Value::Bool(true));
    map.renderer.set_feature_state("shapes", None, "1", &hover);
    assert_eq!(BTreeSet::from([(north_west(), 0)]), map.frame().1);
    assert!(map.frame().1.is_empty());
}

/// A line translated in the viewport, which draws by the bearing, in the
/// north-west tile.
const VIEWPORT_SHIFTED_LINE: &str = r#"{
    "version": 8,
    "sources": {
        "shifted": {"type": "geojson", "data": {"type": "LineString", "coordinates": [[-80, 50], [-70, 60]]}}
    },
    "layers": [{
        "id": "shifted",
        "type": "line",
        "source": "shifted",
        "paint": {"line-width": 8, "line-translate": [20, 0], "line-translate-anchor": "viewport"}
    }]
}"#;

/// A rotation keeps the drapes of layers that draw the same at every
/// bearing.
#[test]
fn a_rotation_keeps_drapes_that_draw_nothing_by_the_bearing() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
    for bearing in [1.0, 2.0] {
        map.transform.jump_to(&CameraOptions { bearing: Some(bearing), ..Default::default() });
        let (_, drawn) = map.frame();
        assert_eq!(drapes, map.textured(), "the same drapes cover the view at bearing {bearing}");
        assert!(drawn.is_empty(), "drapes drawn at bearing {bearing}: {drawn:?}");
    }
}

/// Every turn of the map draws again the drapes of a stack with a layer
/// that draws by the bearing, and only them: here the stack over a circle,
/// while the stack under it keeps its drapes.
#[test]
fn a_rotation_draws_again_only_a_stack_that_draws_by_the_bearing() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
    map.add_layer(CIRCLE, None);
    map.add_layer(VIEWPORT_SHIFTED_LINE, None);
    map.settle();
    let shifted = BTreeSet::from([(north_west(), 1)]);
    assert_eq!(&shifted | &drapes, map.textured());
    for bearing in [1.0, 2.0] {
        map.transform.jump_to(&CameraOptions { bearing: Some(bearing), ..Default::default() });
        let (_, drawn) = map.frame();
        assert_eq!(shifted, drawn, "the drapes drawn at bearing {bearing}");
    }
    assert!(map.frame().1.is_empty(), "a drape drawn again at the same bearing");
}

/// A hillshade of the DEM's tiles, lit from the viewport, as hillshades are
/// by default.
const VIEWPORT_LIT_HILLSHADE: &str = r#"{
    "version": 8,
    "sources": {
        "hills": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 10}
    },
    "layers": [{"id": "hillshade", "type": "hillshade", "source": "hills"}]
}"#;

/// A fill that multiplies what is under it, in the north-west tile.
const MULTIPLIED_FILL: &str = r#"{
    "version": 8,
    "sources": {
        "multiplied": {
            "type": "geojson",
            "data": {"type": "Polygon", "coordinates": [[[-80, 50], [-70, 50], [-70, 60], [-80, 50]]]}
        }
    },
    "layers": [{"id": "multiplied", "type": "fill", "source": "multiplied", "paint": {"fill-layer-blend": "multiply"}}]
}"#;

/// A hillshade lit from the viewport has a stack to itself, whose drapes
/// hold its slopes, which the surface shades with the light of the frame's
/// bearing: a rotation draws no drape again.
#[test]
fn a_rotation_keeps_the_drapes_of_a_hillshade_lit_from_the_viewport() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
    map.add_layer(VIEWPORT_LIT_HILLSHADE, Some("fill"));
    map.settle();
    let stacks = |stack| drapes.iter().map(move |&(tile, _)| (tile, stack));
    let expected: BTreeSet<_> = stacks(0).chain(stacks(1)).chain([(north_west(), 2)]).collect();
    assert_eq!(expected, map.textured(), "the stacks under, of and over the hillshade");
    for bearing in [1.0, 2.0] {
        map.transform.jump_to(&CameraOptions { bearing: Some(bearing), ..Default::default() });
        let (_, drawn) = map.frame();
        assert!(drawn.is_empty(), "drapes drawn at bearing {bearing}: {drawn:?}");
    }
}

/// A layer that blends over a hillshade lit from the viewport blends over
/// its colors, in its stack: a rotation draws that stack's drapes again.
#[test]
fn a_blend_over_a_hillshade_lit_from_the_viewport_draws_its_stack_by_the_bearing() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
    map.add_layer(VIEWPORT_LIT_HILLSHADE, Some("fill"));
    map.add_layer(MULTIPLIED_FILL, None);
    map.settle();
    assert_eq!(drapes, map.textured(), "one stack");
    for bearing in [1.0, 2.0] {
        map.transform.jump_to(&CameraOptions { bearing: Some(bearing), ..Default::default() });
        let (_, drawn) = map.frame();
        assert_eq!(drapes, drawn, "the drapes drawn at bearing {bearing}");
    }
}

/// The center at `longitude`, at 10° S.
fn center(longitude: f64) -> CameraOptions {
    CameraOptions { center: Some(LatLng::new(-10.0, longitude).unwrap()), ..Default::default() }
}

/// Once the map's center crosses the antimeridian, the terrain tiles over
/// the same ground are in another world copy, and the map draws what a map
/// that started there draws.
#[test]
fn a_pan_across_the_antimeridian_draws_what_a_new_map_draws() {
    let Some(mut map) = DrapedMap::new(Tiles::default()) else {
        return;
    };
    map.transform.jump_to(&center(179.0));
    map.settle();
    let before = map.textured();
    map.transform.jump_to(&center(-179.0));
    map.settle();
    // The tiles over the ground in view before, a world copy west.
    let shifted = before.iter().map(|&(tile, stack)| (OverscaledTileID { wrap: tile.wrap - 1, ..tile }, stack));
    assert!(
        shifted.clone().any(|drape| map.textured().contains(&drape)),
        "the view crossed into another world copy: {before:?}, then {:?}",
        map.textured()
    );

    let Some(mut fresh) = DrapedMap::new(Tiles::default()) else {
        return;
    };
    fresh.transform.jump_to(&center(-179.0));
    fresh.settle();
    assert!(map.image() == fresh.image(), "the map draws otherwise than a map that started there");
}

/// The rendering stats count the drapes, their bytes and what each frame
/// drew of them.
#[test]
fn the_rendering_stats_count_the_drapes() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
    map.edit(Edit::SetLayerProperty {
        layer: "line".to_owned(),
        name: "line-color".to_owned(),
        value: serde_json::json!("green"),
    });
    map.frame();
    let terrain = map.renderer.rendering_stats().terrain;
    // The DEM's tiles of 256 pixels make terrain tiles of 512, and drapes
    // of 1024 texels a side, with their mips, which draw with a depth and
    // stencil of that side, and no layer composite.
    assert_eq!(drapes.len(), terrain.drapes);
    assert_eq!(drapes.len() as u64 * DRAPE_BYTES + DEPTH_STENCIL_BYTES, terrain.mem_drapes as u64);
    assert_eq!(drapes.len(), terrain.drapes_drawn, "the line's stack");

    map.frame();
    let at_rest = map.renderer.rendering_stats().terrain;
    assert_eq!(0, at_rest.drapes_drawn);
    assert_eq!(terrain.mem_drapes, at_rest.mem_drapes);
}

/// A drape of 1024 texels a side, with its mips, and a depth and stencil of
/// that side.
const DRAPE_BYTES: u64 = 4 * 1024 * 1024 * 4 / 3;
const DEPTH_STENCIL_BYTES: u64 = 4 * 1024 * 1024;

/// A memory limit under what the drapes would hold halves the drapes of the
/// tiles farthest from the view's center, last in the cover, which draw
/// again once, smaller; every tile keeps its drape. Raised again, it lets
/// them draw again at their whole side.
#[test]
fn a_memory_limit_halves_the_drapes_of_the_farthest_tiles_first() {
    let Some((mut map, drapes)) = settled(Tiles::default()) else {
        return;
    };
    let whole: Vec<_> = map.sides().into_iter().map(|(tile, _)| (tile, 1024)).collect();
    assert_eq!(whole, map.sides());

    map.terrain.drape_memory_limit = Some(4 * DRAPE_BYTES + DEPTH_STENCIL_BYTES - 1);
    let (_, drawn) = map.frame();
    let sides = map.sides();
    let (last, _) = *sides.last().unwrap();
    assert_eq!([&whole[..3], &[(last, 512)]].concat(), sides, "the farthest tile's drape halved");
    assert_eq!(BTreeSet::from([(last, 0)]), drawn, "the halved drape draws again");
    let terrain = map.renderer.rendering_stats().terrain;
    assert_eq!(drapes.len(), terrain.drapes, "every tile keeps its drape");
    assert!((terrain.mem_drapes as u64) < 4 * DRAPE_BYTES + DEPTH_STENCIL_BYTES, "{terrain:?}");
    assert!(map.frame().1.is_empty(), "the drapes are drawn again once only");

    map.terrain.drape_memory_limit = None;
    assert_eq!(BTreeSet::from([(last, 0)]), map.frame().1);
    assert_eq!(whole, map.sides());
}
