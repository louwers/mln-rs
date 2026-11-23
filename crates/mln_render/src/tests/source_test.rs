use std::sync::Arc;
use std::time::Duration;

use mln_map_types::{
    CameraOptions, ConstrainMode, MapDebugOptions, MapMode, TileLodMode, TransformState, ViewportMode,
};
use mln_runtime::map::transform::Transform;
use mln_style::Source;
use mln_style::layer::Layer;
use mln_style::layer_type::LayerStyle;
use mln_style_types::ProjectionDefinition;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::constants::TILE_SIZE_I;
use mln_util::geo::LatLng;
use mln_util::logging::{Event, EventSeverity, Log, Observer as LogObserver};
use mln_util::pool::TaskGroup;
use mln_util::range::Range;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use parking_lot::Mutex;

use crate::Registry;
use crate::erased_layer::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::render_source::RenderSource;
use crate::source::TileNecessity;
use crate::tests::fixture_layer::{Dot, Shade, Tint};
use crate::tests::scripted_origin::{self, Call, Dem, Geometry, Raster, Reply, Script, ScriptedLoad};
use crate::tile::source_tiles::SourceTiles;
use crate::tile::store::{StoreEvent, TileStore};
use crate::tile_parameters::TileParameters;

struct SourceTest {
    transform: Transform,
    thread_pool: TaskGroup,
}

/// The tile parameters for `map_mode`, from a transform state the caller
/// owns.
fn tile_parameters(state: &TransformState, map_mode: MapMode) -> TileParameters<'_> {
    TileParameters {
        pixel_ratio: 1.0,
        debug_options: MapDebugOptions::NO_DEBUG,
        view: state.view(),
        min_zoom: state.min_zoom(),
        max_zoom: state.limits().max_zoom,
        file_source: None,
        mode: map_mode,
        prefetch_zoom_delta: 0,
        tile_lod_min_radius: 3.0,
        tile_lod_scale: 1.0,
        tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
        tile_lod_zoom_shift: 0.0,
        tile_lod_mode: TileLodMode::Default,
        subdivision_granularity: SubdivisionGranularitySetting::none(),
        cpu_rasterizer: false,
        global_state: None,
        fast_pfor_enabled: false,
        font_faces: None,
        glyph_pbfs: true,
        placement_commits: 0,
    }
}

impl SourceTest {
    fn new() -> Self {
        let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(Size::new(512, 512)).unwrap();
        transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(0.0), ..Default::default() });
        SourceTest { transform, thread_pool: TaskGroup::new(&crate::tests::pool()) }
    }

    fn tile_parameters_with(&self, map_mode: MapMode) -> TileParameters<'_> {
        tile_parameters(self.transform.state(), map_mode)
    }

    /// A tile store whose jobs run on the thread pool.
    fn store(&self) -> TileStore {
        crate::tests::pool_store(&self.thread_pool)
    }
}

impl Drop for SourceTest {
    fn drop(&mut self) {
        self.thread_pool.wait();
    }
}

/// Answers the loads that `script`'s tiles began with what `reply` makes
/// of each tile, and runs `store`'s updates as the renderer's, until
/// `on_event` returns true for what the store reports.
fn run_until(
    store: &mut TileStore,
    script: &Script,
    mut reply: impl FnMut(&OverscaledTileID) -> Reply,
    mut on_event: impl FnMut(&mut TileStore, StoreEvent) -> bool,
) {
    let deadline = mln_util::time::Instant::now() + Duration::from_secs(30);
    loop {
        assert!(mln_util::time::Instant::now() < deadline, "the test didn't end");
        script.answer(&mut reply);
        crate::tests::update(store);
        let mut ended = false;
        for event in store.take_events() {
            ended |= on_event(store, event);
        }
        if ended {
            return;
        }
        std::thread::yield_now();
    }
}

/// The three kinds of tiled sources.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tiled {
    Raster,
    RasterDEM,
    Vector,
}

impl Tiled {
    /// A tint, shade or dot (source-layer "water") layer "id" on "source",
    /// as evaluated values.
    fn layers(self) -> Vec<Arc<dyn ErasedEvaluatedLayer>> {
        let properties: Arc<dyn ErasedEvaluatedLayer> = match self {
            Tiled::Raster => {
                let layer = Layer::new(Registry::with_builtins().style(), "tint", "id", "source").unwrap();
                Arc::new(EvaluatedLayer::<Tint>::new(LayerStyle::recover(layer.style()).unwrap()))
            }
            Tiled::RasterDEM => {
                let layer = Layer::new(Registry::with_builtins().style(), "shade", "id", "source").unwrap();
                Arc::new(EvaluatedLayer::<Shade>::new(LayerStyle::recover(layer.style()).unwrap()))
            }
            Tiled::Vector => {
                let mut layer = Layer::new(Registry::with_builtins().style(), "dot", "id", "source").unwrap();
                layer.set_source_layer("water");
                Arc::new(EvaluatedLayer::<Dot>::new(LayerStyle::recover(layer.style()).unwrap()))
            }
        };
        vec![properties]
    }

    /// The source "source" of tiles 512 pixels wide, of zoom levels
    /// `zoom_range`, which load as `script` says.
    fn source(self, script: &Script, zoom_range: Range<u8>) -> Source {
        match self {
            Tiled::Raster => scripted_origin::source::<Raster>("source", script, TILE_SIZE_I, zoom_range),
            Tiled::RasterDEM => scripted_origin::source::<Dem>("source", script, TILE_SIZE_I, zoom_range),
            Tiled::Vector => scripted_origin::source::<Geometry>("source", script, TILE_SIZE_I, zoom_range),
        }
    }
}

/// The zoom levels of a tileset that doesn't name them.
fn all_zooms() -> Range<u8> {
    Range::new(0, 22)
}

/// The tiles of `loads`, in the order the loads began.
fn loaded_tiles(loads: &[ScriptedLoad]) -> Vec<CanonicalTileID> {
    loads.iter().map(|load| load.tile.canonical).collect()
}

/// A tile that leaves the view while it loads goes on loading, and is the
/// one the view gets back when it turns back, as a map rotated back and
/// forth does at its corners: its load is neither stopped nor begun again.
#[test]
fn tile_out_of_view_keeps_loading() {
    let mut test = SourceTest::new();
    test.transform.resize(Size::new(1024, 128)).unwrap();
    let script = Script::default();
    let layers = Tiled::Vector.layers();
    let source = Tiled::Vector.source(&script, all_zooms());
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let mut loads = Vec::new();
    // The tiles loaded so far, after an update with the camera at `bearing`.
    let mut turn = |test: &mut SourceTest, bearing: f64, needs_relayout: bool| {
        test.transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(20.0, 20.0).unwrap()),
            zoom: Some(4.0),
            bearing: Some(bearing),
            ..Default::default()
        });
        let parameters = test.tile_parameters_with(MapMode::Continuous);
        render_source.update(&mut store, source.clone(), &layers, true, needs_relayout, &parameters);
        loads.extend(script.take_loads());
        assert!(loads.iter().all(|load| !load.is_stopped()), "no load stops");
        loaded_tiles(&loads)
    };

    let unturned = turn(&mut test, 0.0, true);
    let turned = turn(&mut test, 90.0, false);
    assert!(turned.len() > unturned.len(), "turning shows other tiles");
    let back = turn(&mut test, 0.0, false);
    assert_eq!(turned, back, "no tile loads again");
}

/// A tile that the view leaves far behind while it loads is made optional,
/// which stops its load, before it goes.
#[test]
fn a_tile_left_far_behind_stops_loading_before_it_goes() {
    let mut test = SourceTest::new();
    test.transform.resize(Size::new(256, 256)).unwrap();
    let script = Script::default();
    let layers = Tiled::Vector.layers();
    let source = Tiled::Vector.source(&script, all_zooms());
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let mut jump = |test: &mut SourceTest, longitude: f64, needs_relayout: bool| {
        test.transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(0.0, longitude).unwrap()),
            zoom: Some(6.0),
            ..Default::default()
        });
        let parameters = test.tile_parameters_with(MapMode::Continuous);
        render_source.update(&mut store, source.clone(), &layers, true, needs_relayout, &parameters);
    };

    jump(&mut test, 0.0, true);
    let loads = script.take_loads();
    script.take_calls();
    jump(&mut test, 90.0, false);
    let calls = script.take_calls();
    let dropped: Vec<OverscaledTileID> = calls
        .iter()
        .filter_map(|call| match call {
            Call::Dropped(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert!(!dropped.is_empty(), "the view left tiles far behind");
    for id in &dropped {
        let optional = calls.iter().position(|call| *call == Call::SetNecessity(*id, TileNecessity::Optional));
        let gone = calls.iter().position(|call| *call == Call::Dropped(*id));
        assert!(optional.is_some_and(|optional| Some(optional) < gone), "{id:?} in {calls:?}");
    }
    assert!(loads.iter().filter(|load| dropped.contains(&load.tile)).all(ScriptedLoad::is_stopped));
}

/// The loads of a view's tiles stand in the order the view needs them:
/// the coarse cover's, then the ideal cover's from the centre outwards. A
/// tile that leaves the view while it loads, and goes on loading next to it,
/// stands behind them all until the view comes back to it.
#[test]
fn tile_requests_follow_the_view() {
    const UNWANTED: u32 = u32::MAX;
    let mut test = SourceTest::new();
    // One ideal tile wide and several tall, so that a move of one tile to
    // the side leaves every ideal tile behind, next to the new ones.
    test.transform.resize(Size::new(256, 1024)).unwrap();
    let script = Script::default();
    let layers = Tiled::Vector.layers();
    let source = Tiled::Vector.source(&script, all_zooms());
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let mut loads = Vec::new();
    // The tiles loaded so far and where their loads stand now, after an
    // update with the camera at `center`.
    let mut look_at = |test: &mut SourceTest, center: LatLng, needs_relayout: bool| {
        test.transform.jump_to(&CameraOptions { center: Some(center), zoom: Some(6.0), ..Default::default() });
        let mut parameters = test.tile_parameters_with(MapMode::Continuous);
        parameters.prefetch_zoom_delta = 4;
        render_source.update(&mut store, source.clone(), &layers, true, needs_relayout, &parameters);
        loads.extend(script.take_loads());
        let orders: Vec<u32> = loads.iter().map(ScriptedLoad::order).collect();
        (loaded_tiles(&loads), orders)
    };

    // The middle of the zoom 6 tile column `x`.
    let column = |x: f64| LatLng::new(20.0, -180.0 + (x + 0.5) * 360.0 / 64.0).unwrap();
    let here = column(35.0);
    let (tiles, first) = look_at(&mut test, here, true);
    let coarse: Vec<u32> = tiles.iter().zip(&first).filter(|(tile, _)| tile.z == 2).map(|(_, order)| *order).collect();
    let ideal: Vec<u32> = tiles.iter().zip(&first).filter(|(tile, _)| tile.z == 6).map(|(_, order)| *order).collect();
    assert_eq!(coarse.len() + ideal.len(), tiles.len());
    assert!(!coarse.is_empty() && ideal.len() > 1, "the view has coarse tiles and several ideal ones");
    assert!(coarse.iter().all(|order| *order > 0), "a tile's load comes after the requests that set no order");
    assert!(coarse.iter().max() < ideal.iter().min(), "the coarse tiles come first");
    assert!(ideal.is_sorted_by(|a, b| a < b), "the ideal tiles come in the cover's order");
    assert!(ideal.iter().all(|order| *order < UNWANTED));

    let first_tiles = tiles;
    let (tiles, second) = look_at(&mut test, column(36.0), false);
    assert!(tiles.len() > first.len(), "the new view needs other tiles");
    for ((tile, before), after) in first_tiles.iter().zip(&first).zip(&second) {
        if tile.z == 6 {
            assert_eq!(*after, UNWANTED, "the tiles left behind come last");
        } else {
            assert_eq!(after, before, "the coarse tile stays in view");
        }
    }
    assert!(second[first.len()..].iter().all(|order| *order < UNWANTED));

    let (back_tiles, back) = look_at(&mut test, here, false);
    assert_eq!(back_tiles, tiles, "no tile loads again");
    assert_eq!(back[..first.len()], first, "the first view's tiles stand as they did");
    assert!(back[first.len()..].iter().all(|order| *order == UNWANTED));
}

type Records = Arc<Mutex<Vec<((EventSeverity, Event, String), bool)>>>;

struct Recorder(Records);

impl LogObserver for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, _code: i64, msg: &str) -> bool {
        self.0.lock().push(((severity, event, msg.to_owned()), false));
        true
    }
}

/// Captures this thread's log records.
struct FixtureLog {
    messages: Records,
}

impl FixtureLog {
    fn new() -> Self {
        let messages: Records = Arc::new(Mutex::new(Vec::new()));
        let _ = Log::set_thread_observer(Some(Box::new(Recorder(Arc::clone(&messages)))));
        FixtureLog { messages }
    }

    fn count(&self, severity: EventSeverity, event: Event, msg: &str) -> usize {
        let mut count = 0;
        for (m, checked) in self.messages.lock().iter_mut() {
            if m.0 == severity && m.1 == event && m.2 == msg {
                *checked = true;
                count += 1;
            }
        }
        count
    }

    fn unchecked_count(&self) -> usize {
        self.messages.lock().iter().filter(|(_, checked)| !checked).count()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}

// gtest: Source.SetMaxParentOverscaleFactor
#[test]
fn set_max_parent_overscale_factor() {
    let mut test = SourceTest::new();
    test.transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(8.0), ..Default::default() });
    let log = FixtureLog::new();

    let script = Script::default();
    let layers = Tiled::Raster.layers();
    let source = Tiled::Raster.source(&script, all_zooms());
    let mut common = *source.common();
    assert_eq!(None, common.max_overscale_factor);
    common.max_overscale_factor = Some(3);
    let style_source = source.with_common(common);

    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&style_source, &mut store);
    let parameters = test.tile_parameters_with(MapMode::Continuous);
    render_source.update(&mut store, style_source.clone(), &layers, true, true, &parameters);

    let loaded_z5 = std::cell::Cell::new(false);
    let reply = |id: &OverscaledTileID| {
        // No tiles above zoom level 5 load.
        assert!(id.canonical.z >= 5, "loaded z{}", id.canonical.z);
        loaded_z5.set(loaded_z5.get() || id.canonical.z == 5);
        // No tile is there, so the source loads parents in their place. An
        // empty raster tile would be drawn, empty, instead.
        Reply::Missing
    };
    // Each tile that changes updates the source again, until a tile of zoom
    // level 5 has loaded.
    run_until(&mut store, &script, reply, |store, event| {
        if !matches!(event, StoreEvent::Changed) {
            return false;
        }
        render_source.update(store, style_source.clone(), &layers, true, true, &parameters);
        loaded_z5.get()
    });

    assert_eq!(
        1,
        log.count(EventSeverity::Warning, Event::Style, "Parent tile overscale factor will cap prefetch delta to 3")
    );
    assert_eq!(0, log.unchecked_count());
}

// gtest: Source.RasterDEMTileRoundsCoveringZoom
#[test]
fn raster_dem_tile_rounds_covering_zoom() {
    let mut test = SourceTest::new();
    test.transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(4.6), ..Default::default() });

    let script = Script::default();
    let layers = Tiled::RasterDEM.layers();
    let source = Tiled::RasterDEM.source(&script, all_zooms());
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let parameters = test.tile_parameters_with(MapMode::Continuous);
    render_source.update(&mut store, source.clone(), &layers, true, true, &parameters);

    let mut zooms = Vec::new();
    let reply = |id: &OverscaledTileID| {
        zooms.push(id.canonical.z);
        Reply::Empty
    };
    run_until(&mut store, &script, reply, |_, event| matches!(event, StoreEvent::Changed));

    assert_ne!(zooms, Vec::<u8>::new());
    for z in zooms {
        assert_eq!(5, z);
    }
}

/// A vector source drawn with the camera of a map, whose tiles' loads are
/// answered only when the test says so.
struct TurningSource {
    test: SourceTest,
    script: Script,
    /// The loads answered so far.
    answered: usize,
    source: Source,
    store: TileStore,
    render_source: Box<dyn RenderSource>,
    layers: Vec<Arc<dyn ErasedEvaluatedLayer>>,
    prefetch_zoom_delta: u8,
    latitude: f64,
    /// The placements committed before the next update.
    placement_commits: u64,
}

impl TurningSource {
    fn set_max_overscale_factor(&mut self, factor: Option<u8>) {
        let mut common = *self.source.common();
        common.max_overscale_factor = factor;
        self.source = self.source.with_common(common);
    }

    fn new(projection: &str, size: Size, min_zoom: u8) -> Self {
        let mut test = SourceTest::new();
        test.transform.resize(size).unwrap();
        test.transform.set_projection_definition(&ProjectionDefinition::from_name(projection));
        let script = Script::default();
        let source = Tiled::Vector.source(&script, Range::new(min_zoom, 14));
        let mut store = crate::tests::tile_store(&test.thread_pool, Box::new(crate::tile::tile_workers::InlineWorkers));
        let render_source = crate::tests::create_render_source(&source, &mut store);
        TurningSource {
            test,
            script,
            answered: 0,
            source,
            store,
            render_source,
            layers: Tiled::Vector.layers(),
            prefetch_zoom_delta: 0,
            latitude: 0.0,
            placement_commits: 0,
        }
    }

    fn pyramid(&self) -> &SourceTiles {
        self.render_source.tiles().and_then(|key| self.store.source(key)).expect("the source's tiles")
    }

    /// Moves the camera and updates the source, as a frame would.
    fn update(&mut self, longitude: f64, zoom: f64) {
        self.test.transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(self.latitude, longitude).unwrap()),
            zoom: Some(zoom),
            ..Default::default()
        });
        self.update_rendering(true);
    }

    /// Updates the source at this camera, as a frame does in which its
    /// layers are visible, or (with `false`) hidden.
    fn update_rendering(&mut self, needs_rendering: bool) {
        let mut parameters = self.test.tile_parameters_with(MapMode::Continuous);
        parameters.prefetch_zoom_delta = self.prefetch_zoom_delta;
        parameters.placement_commits = self.placement_commits;
        let (store, source, layers) = (&mut self.store, self.source.clone(), &self.layers);
        self.render_source.update(store, source, layers, needs_rendering, false, &parameters);
    }

    /// How many tiles the source caches, each checked to wait behind the
    /// view's: its jobs and its loads.
    fn cached_unwanted(&self) -> usize {
        let unwanted = crate::tile::tile::TileLoadOrder::UNWANTED;
        let cached: Vec<_> = self.pyramid().slots().filter(|slot| slot.place().is_cached()).collect();
        for slot in &cached {
            assert_eq!(slot.place().order(), unwanted, "tile {}'s jobs", slot.tile.id);
            assert_eq!(slot.tile.load.order.get(), unwanted.value(), "tile {}'s load", slot.tile.id);
        }
        cached.len()
    }

    /// Updates the source at this camera until every tile it asks for has
    /// loaded.
    fn load(&mut self, longitude: f64, zoom: f64) {
        self.update(longitude, zoom);
        for _ in 0..100 {
            // A load the tile stopped waiting for is never answered.
            for load in self.script.take_loads().into_iter().filter(|load| !load.is_stopped()) {
                self.answered += 1;
                load.reply(Reply::Empty);
            }
            self.test.thread_pool.wait();
            crate::tests::update(&mut self.store);
            self.update(longitude, zoom);
            if self.pyramid().is_loaded() {
                return;
            }
        }
        panic!("the tiles did not load");
    }

    /// The pyramid's tiles of zoom `z`.
    fn tiles_at(&self, z: u8) -> Vec<CanonicalTileID> {
        self.pyramid().cover().map(|tile| tile.id).filter(|id| id.canonical.z == z).map(|id| id.canonical).collect()
    }

    /// The tiles still loading that no rendered tile stands in for.
    fn bare_tiles(&self) -> Vec<OverscaledTileID> {
        let rendered: Vec<CanonicalTileID> = self.pyramid().rendered().map(|rendered| rendered.id.canonical).collect();
        self.pyramid()
            .cover()
            .filter(|tile| {
                !tile.is_renderable()
                    && !rendered
                        .iter()
                        .any(|parent| *parent == tile.id.canonical || tile.id.canonical.is_child_of(parent))
            })
            .map(|tile| tile.id)
            .collect()
    }
}

/// A tile that enters the cache, whether the view left it or the source's
/// layers were hidden, waits behind the view's tiles: its jobs, and its
/// request should it revalidate.
#[test]
fn cached_tiles_wait_behind_the_view() {
    let mut panned = TurningSource::new("mercator", Size::new(512, 512), 0);
    panned.load(0.0, 3.0);
    assert_eq!(0, panned.cached_unwanted());
    // The tiles the view left are held while their symbols fade out, for
    // two placements.
    for _ in 0..3 {
        panned.update(120.0, 3.0);
        panned.placement_commits += 1;
    }
    assert!(panned.cached_unwanted() > 0, "the tiles the view left");

    let mut hidden = TurningSource::new("mercator", Size::new(512, 512), 0);
    hidden.load(0.0, 3.0);
    let cover = hidden.pyramid().cover().count();
    hidden.update_rendering(false);
    assert_eq!(0, hidden.pyramid().cover().count());
    assert_eq!(cover, hidden.cached_unwanted(), "the hidden source's tiles");
}

/// A tile that comes back from the cache shows its data while it
/// revalidates if its origin's times allow it; a tile whose data may no
/// longer be shown, such as stale data that must revalidate, goes, and loads
/// anew.
#[test]
fn a_cached_tile_shows_its_data_only_while_caching_rules_allow() {
    use mln_storage::response::Caching;
    use mln_util::chrono::Timestamp;

    for (expires, must_revalidate, reused) in [(i64::MAX, false, true), (0, false, true), (0, true, false)] {
        let mut turned = TurningSource::new("mercator", Size::new(512, 512), 0);
        let caching = Caching { expires: Some(Timestamp::from_secs(expires)), must_revalidate };
        turned.script.set_times(crate::source::TileTimes {
            modified: None,
            expires: caching.expires,
            usable_until: caching.usable_until(),
        });
        turned.load(0.0, 3.0);
        let shown: Vec<_> = turned.pyramid().cover().map(|tile| tile.id).collect();
        for _ in 0..3 {
            turned.update(120.0, 3.0);
            turned.placement_commits += 1;
        }
        assert!(turned.cached_unwanted() > 0, "the tiles the view left");
        turned.update(0.0, 3.0);
        let pyramid = turned.pyramid();
        let renderable = shown.iter().all(|id| pyramid.tile(id).is_some_and(crate::tile::tile::Tile::is_renderable));
        assert_eq!(reused, renderable, "data that expires at {expires}, must revalidate: {must_revalidate}");
    }
}

/// A tile the view leaves is held for its symbols' fade from the update that
/// leaves it until two placements have committed since, however many
/// updates come in between, and the next update drops it.
#[test]
fn a_tile_the_view_leaves_is_held_for_two_placements() {
    let mut turned = TurningSource::new("mercator", Size::new(512, 512), 0);
    turned.placement_commits = 5;
    turned.load(0.0, 3.0);
    assert!(!turned.store.has_fading_tiles());
    turned.update(120.0, 3.0);
    assert!(turned.store.has_fading_tiles(), "held from the update that leaves it");
    turned.update(120.0, 3.0);
    assert!(turned.store.has_fading_tiles(), "held while no placement commits");
    turned.placement_commits += 1;
    turned.update(120.0, 3.0);
    assert!(turned.store.has_fading_tiles(), "held after one placement");
    turned.placement_commits += 1;
    turned.update(120.0, 3.0);
    assert!(!turned.store.has_fading_tiles(), "dropped after two");
}

/// A view that zooms in draws the loaded parent in place of its children
/// while they load, and the children once they have.
#[test]
fn a_parent_draws_while_its_child_loads() {
    let mut zoomed = TurningSource::new("mercator", Size::new(512, 512), 0);
    zoomed.load(0.0, 0.0);
    // Not those held while what their layers placed fades out.
    let rendered = |source: &TurningSource| -> Vec<CanonicalTileID> {
        let rendered = source.pyramid().rendered().filter(|tile| !tile.hold_for_fade);
        let mut ids: Vec<CanonicalTileID> = rendered.map(|tile| tile.id.canonical).collect();
        ids.sort();
        ids
    };
    assert_eq!(rendered(&zoomed), [CanonicalTileID::new(0, 0, 0)]);

    zoomed.update(0.0, 1.0);
    let children = zoomed.tiles_at(1);
    assert_eq!(children.len(), 4, "the view's tiles at zoom 1");
    assert!(zoomed.pyramid().cover().filter(|tile| tile.id.canonical.z == 1).all(|tile| !tile.is_renderable()));
    assert_eq!(rendered(&zoomed), [CanonicalTileID::new(0, 0, 0)], "the parent, while its children load");

    zoomed.load(0.0, 1.0);
    let mut children = children;
    children.sort();
    assert_eq!(rendered(&zoomed), children, "the children, once they have loaded");
}

/// A globe turned to its far side before any tile there has loaded draws the
/// source's first level in their place.
fn globe_far_side_falls_back_to_the_base_level(size: Size, min_zoom: u8, zoom: f64) {
    let mut turning = TurningSource::new("vertical-perspective", size, min_zoom);
    turning.prefetch_zoom_delta = 4;
    turning.load(60.0, zoom);
    assert_eq!(1 << (2 * min_zoom), turning.tiles_at(min_zoom).len(), "the whole first level is loaded");
    assert!(
        turning.pyramid().rendered().all(|rendered| rendered.id.canonical.z > min_zoom),
        "the loaded view draws none of it"
    );

    turning.update(240.0, zoom);
    let loading = turning.pyramid().cover().filter(|tile| !tile.is_renderable()).count();
    assert!(loading > 0, "the far side has tiles to load");
    assert_eq!(Vec::<OverscaledTileID>::new(), turning.bare_tiles());
    assert!(turning.pyramid().rendered().any(|rendered| rendered.id.canonical.z == min_zoom));
}

#[test]
fn globe_far_side_falls_back_to_zoom_zero() {
    globe_far_side_falls_back_to_the_base_level(Size::new(1920, 1080), 0, 5.0);
}

#[test]
fn globe_far_side_falls_back_to_zoom_one() {
    globe_far_side_falls_back_to_the_base_level(Size::new(512, 512), 1, 3.0);
}

/// The tiles past the antimeridian have another wrap than the zoom-0 tile,
/// which stands in for them all the same, and is drawn once.
#[test]
fn globe_fallback_crosses_the_antimeridian() {
    let mut turning = TurningSource::new("vertical-perspective", Size::new(512, 512), 0);
    turning.load(90.0, 2.0);
    assert!(turning.pyramid().rendered().all(|rendered| rendered.id.canonical.z == 2));

    turning.update(150.0, 2.0);
    let wraps = |renderable: bool| -> Vec<i16> {
        let mut wraps: Vec<i16> = turning
            .pyramid()
            .cover()
            .filter(|tile| tile.id.canonical.z == 2 && tile.is_renderable() == renderable)
            .map(|tile| tile.id.wrap)
            .collect();
        wraps.dedup();
        wraps
    };
    assert_eq!((vec![0], vec![1]), (wraps(true), wraps(false)));
    assert_eq!(Vec::<OverscaledTileID>::new(), turning.bare_tiles());
    assert_eq!(1, turning.pyramid().rendered().filter(|rendered| rendered.id.canonical.z == 0).count());
}

/// While the globe blends into Mercator, a tile's flat half is placed by its
/// wrap, so the tiles past the antimeridian don't take the zoom-0 tile of
/// the other wrap.
#[test]
fn globe_fallback_keeps_to_its_wrap_while_blending_into_mercator() {
    let mut turning = TurningSource::new("vertical-perspective", Size::new(512, 512), 0);
    turning.test.transform.set_projection_definition(&ProjectionDefinition::between_names(
        "vertical-perspective",
        "mercator",
        0.5,
    ));
    turning.load(90.0, 2.0);
    assert_eq!(1, turning.tiles_at(0).len());

    turning.update(150.0, 2.0);
    let bare = turning.bare_tiles();
    assert_ne!(Vec::<OverscaledTileID>::new(), bare);
    assert!(bare.iter().all(|id| id.wrap == 1 && id.canonical.z == 2));
    assert_eq!(0, turning.pyramid().rendered().filter(|rendered| rendered.id.canonical.z == 0).count());
}

/// The first level costs a source that starts at zoom 1 the tiles of it that
/// the view doesn't show, and a Mercator map nothing.
#[test]
fn globe_base_level_requests() {
    let mut globe = TurningSource::new("vertical-perspective", Size::new(512, 512), 1);
    globe.load(60.0, 3.0);
    let mut mercator = TurningSource::new("mercator", Size::new(512, 512), 1);
    mercator.load(60.0, 3.0);
    assert_eq!(Vec::<CanonicalTileID>::new(), mercator.tiles_at(1));
    assert_eq!(4, globe.tiles_at(1).len());
    let view = globe.pyramid().cover().count() - 4;
    assert_eq!(view + 4, globe.answered);
}

#[test]
fn globe_base_level_is_released() {
    let mut turning = TurningSource::new("vertical-perspective", Size::new(512, 512), 1);
    turning.load(60.0, 3.0);
    assert_eq!(4, turning.tiles_at(1).len());

    // The planet's radius passes twice the viewport's side between these.
    turning.update(60.0, 3.6);
    assert_eq!(4, turning.tiles_at(1).len());
    turning.update(60.0, 3.7);
    assert_eq!(Vec::<CanonicalTileID>::new(), turning.tiles_at(1));

    turning.update(60.0, 3.0);
    assert_eq!(4, turning.tiles_at(1).len());
    turning.test.transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    turning.update(60.0, 3.0);
    assert_eq!(Vec::<CanonicalTileID>::new(), turning.tiles_at(1));
}

/// The globe is larger away from the equator at the same zoom, so the first
/// level is let go of at a lower zoom there.
#[test]
fn globe_base_level_is_released_sooner_away_from_the_equator() {
    let mut turning = TurningSource::new("vertical-perspective", Size::new(512, 512), 1);
    turning.latitude = 60.0;
    turning.load(60.0, 2.6);
    assert_eq!(4, turning.tiles_at(1).len());

    // At 60 degrees the planet is twice its size at the equator: its radius
    // passes twice the viewport's side a zoom lower.
    turning.update(60.0, 2.7);
    assert_eq!(Vec::<CanonicalTileID>::new(), turning.tiles_at(1));
}

/// A first level of sixteen tiles is not kept, nor one that the search for a
/// loaded parent would not reach.
#[test]
fn globe_base_level_is_not_kept_where_it_is_large_or_out_of_reach() {
    let mut large = TurningSource::new("vertical-perspective", Size::new(512, 512), 2);
    large.load(60.0, 3.0);
    assert_eq!(Vec::<CanonicalTileID>::new(), large.tiles_at(2));

    let mut out_of_reach = TurningSource::new("vertical-perspective", Size::new(512, 512), 1);
    out_of_reach.set_max_overscale_factor(Some(1));
    out_of_reach.load(60.0, 3.0);
    assert_eq!(Vec::<CanonicalTileID>::new(), out_of_reach.tiles_at(1));
    out_of_reach.set_max_overscale_factor(Some(2));
    out_of_reach.load(60.0, 3.0);
    assert_eq!(4, out_of_reach.tiles_at(1).len());
}

/// The DEM of `tile`, a raster-dem tile.
fn dem(tile: &crate::tile::tile::Tile) -> &crate::tile::dem::DemContents {
    match tile.contents() {
        Some(crate::tile::contents::Contents::Dem(bucket)) => bucket,
        _ => panic!("a DEM"),
    }
}

/// Port-only: raster-dem tiles that meet at the antimeridian, in two copies
/// of the world, backfill each other's borders as neighbours in one copy
/// do.
#[test]
fn raster_dem_borders_across_a_world_wrap() {
    use crate::tests::tile::dem_png;

    let mut test = SourceTest::new();
    test.transform.jump_to(&CameraOptions {
        center: Some(LatLng::new(0.0, 180.0).unwrap()),
        zoom: Some(2.0),
        ..Default::default()
    });
    let script = Script::default();
    let layers = Tiled::RasterDEM.layers();
    let source = Tiled::RasterDEM.source(&script, all_zooms());
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let deadline = mln_util::time::Instant::now() + Duration::from_secs(30);
    let mut relayout = true;
    loop {
        // The store backfills the tiles as their DEMs arrive.
        let parameters = test.tile_parameters_with(MapMode::Continuous);
        render_source.update(&mut store, source.clone(), &layers, true, relayout, &parameters);
        relayout = false;
        let key = render_source.tiles().expect("the source's tiles");
        if store.source(key).is_some_and(|source| source.slots().next().is_some()) && render_source.is_loaded(&store) {
            break;
        }
        assert!(mln_util::time::Instant::now() < deadline, "the tiles didn't load");
        script.answer(|id| Reply::Bytes(dem_png(id.canonical.x)));
        crate::tests::update(&mut store);
        std::thread::yield_now();
    }

    let source = render_source.tiles().and_then(|key| store.source(key)).expect("the source's tiles");
    let tile = |x| {
        source
            .slots()
            .map(|slot| &slot.tile)
            .find(|tile| tile.id.canonical == CanonicalTileID::new(2, x, 1))
            .unwrap_or_else(|| panic!("no tile 2/{x}/1"))
    };
    // The east edge of a copy of the world, and the west edge of the copy
    // east of it.
    let (east, west) = (tile(3), tile(0));
    assert_eq!(west.id.wrap, east.id.wrap + 1);
    let (east, west) = (dem(east), dem(west));
    let (east, west) = (east.dem(), west.dem());
    for y in 0..16 {
        assert_eq!(east.get(16, y), west.get(0, y), "row {y} of the east tile's border");
        assert_eq!(west.get(-1, y), east.get(15, y), "row {y} of the west tile's border");
    }
}
