// Port-only: the tile store keeps a tile's data, work and contents apart.
// Data events name their load and never the layout's generation; replies
// apply only to the generation that asked; contents keep the data they were
// built from while newer data lays out; and a tile that leaves and comes
// back, or a source replaced, never takes what was meant for the one before.

use std::sync::Arc;

use feature::{Feature, PropertyMap};
use geo_types::{Geometry, Point};
use mln_map_types::MapMode;
use mln_storage::Data;
use mln_style::layer_type::SourceKind;
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::identity::SimpleIdentity;
use mln_util::pool::TaskGroup;
use serde_json::json;

use super::tile_jobs_test::{Manual, icons, layer, points};
use crate::erased_layer::ErasedEvaluatedLayer;
use crate::query::SourceQueryOptions;
use crate::tests::scripted_origin::{Call, Reply, Retry, Script, ScriptedValue};
use crate::tests::tile::{dem_png, png};
use crate::tests::{OneTile, style, tile_store};
use crate::tile::contents::Contents;
use crate::tile::dem::DemContents;
use crate::tile::inbox::TileEvent;
use crate::tile::job::{DataInput, Job, Run, TileKey};
use crate::tile::pipeline::LayoutWork;
use crate::tile::tile::{TileNecessity, TileOperation};
use crate::tile::tile_workers::{InlineWorkers, TileWorkers};

/// The kind of the scripted value tiles.
const VALUE: SourceKind = SourceKind::Value(mln_style::layer_type::ValueKind::of::<ScriptedValue>());

/// The bytes of a vector tile with a "water" layer.
fn water() -> Data {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("testdata/test/fixtures/map/issue12432/0-0-0.mvt");
    Arc::new(std::fs::read(path).expect("the tile"))
}

fn water_layer(id: &str, color: &str) -> Arc<dyn ErasedEvaluatedLayer> {
    layer(json!({"id": id, "type": "dot", "source-layer": "water", "paint": {"dot-color": color}}))
}

/// Features with ids `ids`, at the middle of the tile.
fn features(ids: &[u64]) -> TileData {
    let features = ids
        .iter()
        .map(|id| Feature {
            geometry: Some(Geometry::Point(Point::new(2048i16, 2048))),
            properties: Arc::new(PropertyMap::new()),
            id: feature::Identifier::Uint(*id),
        })
        .collect();
    TileData::Features(Arc::new(features))
}

struct Test {
    pool: TaskGroup,
}

impl Test {
    fn new() -> Self {
        Test { pool: TaskGroup::new(&crate::tests::pool()) }
    }

    /// Tile 0/0/0 of a source whose tiles load as their script says once
    /// required, whose jobs run on `workers`.
    fn scripted_tile(&self, workers: Box<dyn TileWorkers>) -> OneTile {
        let store = tile_store(&self.pool, workers);
        let id = OverscaledTileID::from_zxy(0, 0, 0);
        OneTile::with_script(store, SourceKind::Geometry, &Script::default(), &[id], MapMode::Static)
    }

    /// Tile 0/0/0 of a source of its own, whose jobs run on `workers`.
    fn tile(&self, workers: Box<dyn TileWorkers>) -> OneTile {
        let store = tile_store(&self.pool, workers);
        OneTile::new(store, SourceKind::Geometry, OverscaledTileID::from_zxy(0, 0, 0), MapMode::Static)
    }
}

impl Drop for Test {
    fn drop(&mut self) {
        self.pool.wait();
    }
}

/// Starts a load of `tile`, as a GeoJSON source's `setData` does, and
/// returns it.
fn start_geojson_load(tile: &mut OneTile) -> u64 {
    tile.with(|tile| {
        let load = tile.next();
        tile.tile_mut().load.current = Some(load);
        load
    })
}

/// The style of a layout job.
fn job_style(run: &Run) -> &Arc<crate::tile::job::SourceStyle> {
    match &run.job {
        Job::Layout(job) => &job.style,
        Job::Raster { .. } | Job::Dem { .. } => panic!("not a layout job"),
    }
}

/// A style edit while a tile's data loads changes the layout's generation,
/// not the load: the data still applies, and lays out with the newest style.
#[test]
fn a_style_edit_while_the_data_loads() {
    let test = Test::new();
    let manual = Manual::default();
    let mut tile = test.scripted_tile(Box::new(manual.clone()));
    let id = tile.id;
    tile.set_style(&style(&[water_layer("water", "red")]));
    tile.set_necessity(&id, TileNecessity::Required);
    let loads = tile.script.take_loads();
    assert_eq!(loads.len(), 1);

    let edited = style(&[water_layer("water", "blue")]);
    tile.set_style(&edited);
    loads[0].reply(Reply::Bytes(water()));
    tile.update();
    assert_eq!(manual.submitted(), 1, "the data applied");
    let run = manual.take();
    assert!(Arc::ptr_eq(job_style(&run), &edited), "with the newest style");
    manual.reply(InlineWorkers::run(run));
    tile.update();
    assert!(tile.shows_layout() && tile.tile().is_complete());
}

/// A tile let go of while its job runs, and made again before the job
/// replies: the new tile's generation is another, so the old reply is
/// dropped and the new tile shows only its own layout.
#[test]
fn eviction_and_revival_while_a_job_is_in_flight() {
    let test = Test::new();
    let manual = Manual::default();
    let mut tile = test.tile(Box::new(manual.clone()));
    let id = tile.id;
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "dot"}))]));
    tile.set_data(Some(features(&[1])));
    tile.update();
    let old = manual.take();

    tile.drop_tile();
    assert!(old.superseded.load(std::sync::atomic::Ordering::Relaxed), "a worker may skip it");
    tile.add(id);
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "dot"}))]));
    tile.set_data(Some(features(&[2])));
    tile.update();
    let new = manual.take();
    assert_eq!(old.id.tile, new.id.tile);
    assert_ne!(old.id.generation, new.id.generation);

    manual.reply(InlineWorkers::run(old));
    tile.update();
    assert!(!tile.shows_layout(), "the old tile's layout isn't the new one's");
    manual.reply(InlineWorkers::run(new));
    tile.update();
    assert!(tile.shows_layout());
    let Some(Contents::Geometry(contents)) = tile.tile().contents() else {
        panic!("a layout");
    };
    let Some(TileData::Features(shown)) = contents.result.feature_index.data() else {
        panic!("features");
    };
    assert_eq!(shown[0].id, feature::Identifier::Uint(2));
}

/// A tile that waits for its images leaves the cover, its style changes,
/// and it comes back: it lays out its newest generation from the start,
/// and the wait of the one before is gone with its leases.
#[test]
fn a_tile_leaving_the_cover_while_waiting_then_edited_then_revived() {
    let test = Test::new();
    let manual = Manual::default();
    let mut tile = test.tile(Box::new(manual.clone()));
    tile.set_style(&style(&[icons(&json!("late"), 1.0)]));
    tile.set_data(Some(points()));
    tile.update();
    manual.run();
    // No sprite yet: the tile waits for its image.
    tile.update();
    assert!(matches!(&tile.layout().work, LayoutWork::Waiting(_)));

    let edited = style(&[icons(&json!("late"), 2.0)]);
    tile.set_style(&edited);
    assert!(matches!(&tile.layout().work, LayoutWork::Outdated), "the wait is gone");
    tile.store.images_mut().add_image(super::tile_jobs_test::image("late"));
    tile.store.images_mut().set_loaded(true);
    tile.update();
    let run = manual.take();
    assert!(Arc::ptr_eq(job_style(&run), &edited));
    let Job::Layout(job) = &run.job else {
        panic!("not a layout job");
    };
    assert!(job.resolved.icons.is_empty(), "a first try, not the old wait's resume");
    manual.reply(InlineWorkers::run(run));
    tile.update();
    manual.run();
    tile.update();
    assert!(tile.shows_layout() && tile.tile().is_complete());
}

/// While new data lays out, the tile shows its old contents, and queries
/// and feature state read the data those were built from. A layout that
/// installs gets the source's feature state before it draws.
#[test]
fn queries_and_feature_state_read_the_shown_contents() {
    let test = Test::new();
    let manual = Manual::default();
    let mut tile = test.tile(Box::new(manual.clone()));
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "dot"}))]));
    tile.set_data(Some(features(&[1])));
    tile.update();
    manual.run();
    tile.update();
    assert!(tile.shows_layout());

    tile.set_data(Some(features(&[2])));
    tile.update();
    assert_eq!(manual.submitted(), 2, "the new data lays out");
    let key = tile.key;
    let query = |tile: &OneTile| -> Vec<feature::Identifier> {
        let source = tile.store.source(key).expect("the source");
        let options = SourceQueryOptions { source_layers: Some(vec![String::new()]), filter: None };
        source.query_source_features(&options, None).unwrap().into_iter().map(|feature| feature.id).collect()
    };
    assert_eq!(query(&tile), [feature::Identifier::Uint(1)], "the shown contents' data");

    // A feature state edit reaches the shown contents, over their data.
    let mut state = mln_style::expression::expr::FeatureState::new();
    state.insert("hover".to_owned(), feature::Value::Bool(true));
    let (source, _) = tile.store.source_mut(key).expect("the source");
    source.info.feature_state.update_state(None, "1", &state);
    source.coalesce_feature_state();
    let version = source.info.feature_state.version();
    let shown_version = |tile: &OneTile| tile.tile().shown.as_ref().expect("contents").feature_state_version;
    // Not rendered, so the coalesce left it.
    assert_eq!(shown_version(&tile), 0);
    tile.with(|tile| {
        let (info, tile) = tile.parts();
        info.feature_state.coalesce_changes(tile.shown.as_mut());
    });
    assert_eq!(shown_version(&tile), version);

    manual.run();
    tile.update();
    assert_eq!(query(&tile), [feature::Identifier::Uint(2)]);
    assert_eq!(shown_version(&tile), version, "the new layout got the state as it installed");
}

/// Data that a GeoJSON source or a custom source sends a tile that went
/// since applies to no tile made in its place: the new tile's load is
/// another.
#[test]
fn source_data_racing_eviction() {
    let test = Test::new();
    let mut tile = test.tile(Box::new(InlineWorkers));
    let id = tile.id;
    let key = tile.key;
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "dot"}))]));
    // A load under way, as a GeoJSON source's `setData` starts.
    let old_load = start_geojson_load(&mut tile);
    tile.drop_tile();
    tile.add(id);
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "dot"}))]));
    let new_load = start_geojson_load(&mut tile);

    let inbox = tile.store.inbox().clone();
    let event = |load, ids: &[u64]| TileEvent::Load {
        tile: TileKey { source: key, id },
        load,
        reply: Box::new(Reply::Tile(features(ids))),
    };
    inbox.send(event(old_load, &[1]));
    tile.update();
    assert!(tile.layout().data.as_ref().is_none(), "the old tile's data goes nowhere");
    inbox.send(TileEvent::Load { tile: TileKey { source: key, id }, load: old_load, reply: Box::new(Reply::Empty) });
    inbox.send(event(new_load, &[2]));
    // One update starts the layout, whose reply the next takes in.
    tile.update();
    tile.update();
    assert!(tile.shows_layout());
    let Some(data) = tile.layout().data.as_ref().and_then(|loaded| loaded.data.clone()) else {
        panic!("data");
    };
    let TileData::Features(features) = data else {
        panic!("features");
    };
    assert_eq!(features[0].id, feature::Identifier::Uint(2));
}

/// A source removed while its tiles load and lay out, and one of the same
/// id added in its place, as when a style is replaced mid-load: what
/// arrives for the old source's tiles reaches none of the new one's.
#[test]
fn style_replaced_mid_load() {
    let test = Test::new();
    let manual = Manual::default();
    let mut tile = test.scripted_tile(Box::new(manual.clone()));
    let id = tile.id;
    let layers = [water_layer("water", "red")];
    tile.set_style(&style(&layers));
    tile.set_necessity(&id, TileNecessity::Required);
    let [load] = <[_; 1]>::try_from(tile.script.take_loads()).ok().expect("one load");
    load.reply(Reply::Bytes(water()));
    tile.update();
    let old_job = manual.take();

    // The new style's source of the same id.
    let params = tile.tile_params();
    let old_key = tile.key;
    tile.store.remove_source(old_key);
    assert!(old_job.superseded.load(std::sync::atomic::Ordering::Relaxed));
    assert!(load.is_stopped(), "the old tile's load goes with it");
    assert!(tile.script.take_calls().contains(&Call::Dropped(id)));
    let script = Script::default();
    let new_key = tile.store.add_source("source".to_owned(), script.origin(SourceKind::Geometry));
    assert_ne!(new_key, old_key, "keys aren't reused");
    crate::testing::seed(&mut tile.store, new_key, &[id], params);
    tile.key = new_key;
    tile.script = script;
    tile.set_style(&style(&layers));

    // The old job's reply, and an old reply sent as the source went.
    manual.reply(InlineWorkers::run(old_job));
    load.reply(Reply::Bytes(water()));
    tile.update();
    assert!(tile.store.source(old_key).is_none());
    assert!(tile.tile().contents().is_none() && tile.layout().data.as_ref().is_none());
    assert!(tile.script.take_loads().is_empty(), "the new tile loads nothing yet");
    assert_eq!(manual.submitted(), 1, "nothing for the new tile yet");

    tile.set_necessity(&id, TileNecessity::Required);
    tile.script.answer(|_| Reply::Bytes(water()));
    tile.update();
    manual.run();
    tile.update();
    assert!(tile.shows_layout() && tile.tile().is_complete());
}

/// A raster-dem tile whose DEM arrives backfills its border from each loaded
/// neighbour, and theirs from it, whichever arrives first; a tile of the
/// first and last row has no neighbours above and below it.
#[test]
fn dem_backfill_as_dems_arrive() {
    let test = Test::new();
    let store = tile_store(&test.pool, Box::new(InlineWorkers));
    let (left, right) = (OverscaledTileID::from_zxy(1, 0, 0), OverscaledTileID::from_zxy(1, 1, 0));
    let mut tiles = OneTile::new(store, SourceKind::RasterDem, right, MapMode::Static);
    tiles.add(left);
    // Each update starts the decode, whose reply the next takes in.
    tiles.set_image(&right, Some(png(16, 16)));
    tiles.update();
    tiles.update();
    assert!(tiles.tile_at(&right).is_renderable());

    tiles.set_image(&left, Some(png(16, 16)));
    tiles.update();
    tiles.update();
    assert_borders_match(&tiles, &left, &right);

    // The world's one tile at zoom 0 is in the first and the last row.
    let store = tile_store(&test.pool, Box::new(InlineWorkers));
    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let mut root_tile = OneTile::new(store, SourceKind::RasterDem, root, MapMode::Static);
    root_tile.set_image(&root, Some(png(16, 16)));
    root_tile.update();
    root_tile.update();
    assert!(root_tile.tile().is_renderable());
}

/// The DEMs of tiles `left` and `right`, neighbours in a row.
fn dems<'a>(tiles: &'a OneTile, left: &OverscaledTileID, right: &OverscaledTileID) -> [&'a DemContents; 2] {
    [left, right].map(|id| match tiles.tile_at(id).contents() {
        Some(Contents::Dem(bucket)) => bucket,
        _ => panic!("a DEM"),
    })
}

/// Asserts that each of the DEMs of `left` and `right` has the other's edge
/// as its border.
fn assert_borders_match(tiles: &OneTile, left: &OverscaledTileID, right: &OverscaledTileID) {
    let [left_dem, right_dem] = dems(tiles, left, right);
    let (left_dem, right_dem) = (left_dem.dem(), right_dem.dem());
    for y in 0..16 {
        assert_eq!(left_dem.get(16, y), right_dem.get(0, y), "row {y} of the left tile's border");
        assert_eq!(right_dem.get(-1, y), left_dem.get(15, y), "row {y} of the right tile's border");
    }
}

/// A DEM that reloads with other heights refills its border from its
/// neighbours, and theirs from it, as at its first load.
#[test]
fn a_reloaded_dem_refills_both_borders() {
    let test = Test::new();
    let store = tile_store(&test.pool, Box::new(InlineWorkers));
    let (left, right) = (OverscaledTileID::from_zxy(1, 0, 0), OverscaledTileID::from_zxy(1, 1, 0));
    let mut tiles = OneTile::new(store, SourceKind::RasterDem, right, MapMode::Static);
    tiles.add(left);
    for id in [left, right] {
        tiles.set_image(&id, Some(dem_png(1)));
        tiles.update();
        tiles.update();
    }
    assert_borders_match(&tiles, &left, &right);
    let edge = |tiles: &OneTile| -> Vec<i32> {
        let [_, right_dem] = dems(tiles, &left, &right);
        (0..16).map(|y| right_dem.dem().get(0, y)).collect()
    };
    let before = edge(&tiles);

    tiles.set_image(&right, Some(dem_png(2)));
    tiles.update();
    tiles.update();
    assert_ne!(edge(&tiles), before, "the reload has other heights");
    assert_borders_match(&tiles, &left, &right);
}

/// At zoom 1 a tile's neighbour across the antimeridian is the row's other
/// tile, in the next world copy: the borders there fill from it.
#[test]
fn dem_borders_across_the_antimeridian_at_zoom_1() {
    let test = Test::new();
    let store = tile_store(&test.pool, Box::new(InlineWorkers));
    let west = OverscaledTileID::with_zxy(1, -1, 1, 1, 0);
    let east = OverscaledTileID::from_zxy(1, 0, 0);
    let mut tiles = OneTile::new(store, SourceKind::RasterDem, east, MapMode::Static);
    tiles.add(west);
    for (id, seed) in [(west, 1), (east, 2)] {
        tiles.set_image(&id, Some(dem_png(seed)));
        tiles.update();
        tiles.update();
    }
    assert_borders_match(&tiles, &west, &east);
}

/// Data that arrives in the inbox starts its tile's job in the update that
/// takes it in, without waiting for another.
#[test]
fn data_from_the_inbox_starts_a_job_at_once() {
    let test = Test::new();
    let manual = Manual::default();
    let mut tile = test.tile(Box::new(manual.clone()));
    let (id, key) = (tile.id, tile.key);
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "dot"}))]));
    let load = start_geojson_load(&mut tile);
    let inbox = tile.store.inbox().clone();
    // From another thread, as a GeoJSON source's tiles cut on a pool answer.
    std::thread::spawn(move || {
        let reply = Box::new(Reply::Tile(features(&[1])));
        inbox.send(TileEvent::Load { tile: TileKey { source: key, id }, load, reply });
    })
    .join()
    .unwrap();
    tile.update();
    assert_eq!(manual.submitted(), 1);
    let run = manual.take();
    let Job::Layout(job) = &run.job else {
        panic!("not a layout job");
    };
    assert!(matches!(&job.data, DataInput::Full { data: Some(_), .. }));
}

/// A relayout that needs less than the layout before it keeps leases on only
/// what it uses: the bins of the images it no longer draws go back to the
/// atlas.
#[test]
fn a_relayout_keeps_only_the_bins_it_uses() {
    let test = Test::new();
    let manual = Manual::default();
    let mut tile = test.tile(Box::new(manual.clone()));
    for id in ["a", "b"] {
        tile.store.images_mut().add_image(super::tile_jobs_test::image(id));
    }
    tile.store.images_mut().set_loaded(true);
    let icon = |id: &str| layer(json!({"id": id, "type": "dot", "layout": {"dot-icon": id}}));
    tile.set_style(&style(&[icon("a"), icon("b")]));
    tile.set_data(Some(points()));
    for _ in 0..2 {
        tile.update();
        manual.run();
    }
    tile.update();
    let icons = |tile: &OneTile| -> (Vec<String>, usize) {
        let Some(Contents::Geometry(contents)) = tile.tile().contents() else {
            panic!("a layout");
        };
        let mut ids: Vec<String> = contents.answer.resolved.icons.keys().cloned().collect();
        ids.sort();
        let atlases = contents.answer.atlases.as_ref().expect("atlases");
        (ids, atlases.images.texture_handles().len())
    };
    assert_eq!(icons(&tile), (vec!["a".to_owned(), "b".to_owned()], 2));

    // The relayout carries both, and needs one.
    tile.set_style(&style(&[icon("a")]));
    tile.update();
    manual.run();
    tile.update();
    assert_eq!(manual.submitted(), 3, "the relayout took one job");
    assert_eq!(icons(&tile), (vec!["a".to_owned()], 1));
}

/// A required tile whose load failed starts parsing again when it is
/// retried: its parse events say `StartParse` once its new request is made,
/// and `EndParse` once its layout shows.
#[test]
fn a_retried_load_starts_parsing_again() {
    use TileOperation::{EndParse, Error, LoadFromNetwork, RequestedFromNetwork, StartParse};

    let test = Test::new();
    let mut tile = test.scripted_tile(Box::new(InlineWorkers));
    let id = tile.id;
    tile.set_style(&style(&[water_layer("water", "red")]));
    tile.set_necessity(&id, TileNecessity::Required);
    tile.script.answer(|_| Reply::Fail("offline".to_owned()));
    tile.update();
    assert_eq!(tile.actions(), [StartParse, RequestedFromNetwork, Error]);
    assert!(tile.tile().is_complete() && !tile.tile().is_pending());

    tile.store.retry_failed();
    assert_eq!(tile.actions(), [RequestedFromNetwork, StartParse]);
    tile.script.answer(|_| Reply::Bytes(water()));
    tile.update();
    tile.update();
    assert_eq!(tile.actions(), [LoadFromNetwork, EndParse]);
    assert!(tile.shows_layout() && tile.tile().is_complete());
}

/// The origin hears when the engine can't lay out or decode what its tile
/// took in, and not when the origin failed the tile itself.
#[test]
fn a_layout_failure_reaches_the_origin() {
    let test = Test::new();
    for kind in [SourceKind::Geometry, SourceKind::Raster] {
        let store = tile_store(&test.pool, Box::new(InlineWorkers));
        let id = OverscaledTileID::from_zxy(0, 0, 0);
        let mut tile = OneTile::with_script(store, kind, &Script::default(), &[id], MapMode::Static);
        tile.set_style(&style(&[water_layer("water", "red")]));
        tile.set_necessity(&id, TileNecessity::Required);
        tile.script.answer(|_| Reply::Fail("offline".to_owned()));
        tile.update();
        assert!(tile.actions().contains(&TileOperation::Error), "{kind:?}");
        assert!(!tile.script.take_calls().contains(&Call::Failed(id)), "{kind:?}: the origin's own failure");

        tile.store.retry_failed();
        tile.script.answer(|_| Reply::Bytes(Arc::new(b"not a tile".to_vec())));
        tile.update();
        tile.update();
        assert!(tile.actions().contains(&TileOperation::Error), "{kind:?}");
        assert_eq!(
            tile.script.take_calls().iter().filter(|call| **call == Call::Failed(id)).count(),
            1,
            "{kind:?}: the engine's failure"
        );
    }
}

/// A reply to a load that is no longer the tile's latest goes nowhere: the
/// origin never sees it, and the tile shows only the latest load's data.
#[test]
fn a_reply_to_an_older_load_is_dropped() {
    let test = Test::new();
    let mut tile = test.scripted_tile(Box::new(InlineWorkers));
    let id = tile.id;
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "dot"}))]));
    tile.set_necessity(&id, TileNecessity::Required);
    let [older] = <[_; 1]>::try_from(tile.script.take_loads()).ok().expect("one load");
    // Optional and required again: the origin begins a newer load.
    tile.set_necessity(&id, TileNecessity::Optional);
    tile.set_necessity(&id, TileNecessity::Required);
    let [newer] = <[_; 1]>::try_from(tile.script.take_loads()).ok().expect("one load");
    tile.script.take_calls();

    older.reply(Reply::Tile(features(&[1])));
    tile.update();
    assert!(!tile.script.take_calls().contains(&Call::Receive(id)), "the origin never sees the older reply");
    assert!(tile.layout().data.is_none());

    newer.reply(Reply::Tile(features(&[2])));
    tile.update();
    tile.update();
    let source = tile.store.source(tile.key).expect("the source");
    let options = SourceQueryOptions { source_layers: Some(vec![String::new()]), filter: None };
    let features = source.query_source_features(&options, None).unwrap();
    assert_eq!(features.into_iter().map(|feature| feature.id).collect::<Vec<_>>(), [feature::Identifier::Uint(2)]);
}

/// Tile 0/0/0 of a source whose tiles load as `script` says, whose load
/// failed.
fn failed_tile(test: &Test, script: &Script, workers: Box<dyn TileWorkers>) -> OneTile {
    let store = tile_store(&test.pool, workers);
    let id = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tile = OneTile::with_script(store, SourceKind::Geometry, script, &[id], MapMode::Static);
    tile.set_style(&style(&[water_layer("water", "red")]));
    tile.set_necessity(&id, TileNecessity::Required);
    tile.script.answer(|_| Reply::Fail("offline".to_owned()));
    tile.update();
    assert!(tile.tile().is_settled(), "a failed tile has done what it can");
    tile
}

/// A retry that begins a load starts the tile over: it hasn't done what it
/// can until the load ends, and then shows what the load brought.
#[test]
fn a_failed_tile_loads_when_retried() {
    let test = Test::new();
    let mut tile = failed_tile(&test, &Script::default(), Box::new(InlineWorkers));
    tile.store.retry_failed();
    assert!(!tile.tile().is_settled(), "the tile loads anew");
    tile.script.answer(|_| Reply::Bytes(water()));
    tile.update();
    tile.update();
    assert!(tile.shows_layout() && tile.tile().is_complete());
}

/// A retry that neither loads nor delivers, as of an origin without
/// retries, leaves a failed tile as it was: it has done what it can.
#[test]
fn a_failed_tile_stays_settled_for_an_origin_without_retry() {
    let test = Test::new();
    let script = Script::default();
    script.set_retry(Retry::Nothing);
    let mut tile = failed_tile(&test, &script, Box::new(InlineWorkers));
    tile.store.retry_failed();
    assert!(tile.script.take_calls().contains(&Call::Retry(tile.id)));
    assert!(tile.tile().is_settled() && tile.tile().is_complete());
    assert!(tile.script.take_loads().is_empty());
}

/// A retry that delivers data without beginning a load starts the tile
/// over too: it hasn't done what it can until that data lays out.
#[test]
fn a_failed_tile_unsettles_for_a_retry_that_delivers() {
    let test = Test::new();
    let script = Script::default();
    script.set_retry(Retry::Deliver);
    let manual = Manual::default();
    let mut tile = failed_tile(&test, &script, Box::new(manual.clone()));
    tile.store.retry_failed();
    assert!(tile.script.take_loads().is_empty(), "no load");
    assert!(!tile.tile().is_settled(), "the delivered data has yet to lay out");
    tile.update();
    manual.run();
    tile.update();
    assert!(tile.shows_layout() && tile.tile().is_complete());
}

/// A tile that starts from the same tile in another world copy shows that
/// tile's contents at once. If its origin takes the copy, it has done what
/// it can and lays the copy's data out for itself; if not, it loads.
#[test]
fn a_world_copy_takes_its_copy() {
    let test = Test::new();
    let script = Script::default();
    let store = tile_store(&test.pool, Box::new(InlineWorkers));
    let loaded = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tiles = OneTile::with_script(store, SourceKind::Geometry, &script, &[loaded], MapMode::Static);
    let red = style(&[water_layer("water", "red")]);
    tiles.set_style(&red);
    tiles.set_necessity(&loaded, TileNecessity::Required);
    script.answer(|_| Reply::Bytes(water()));
    tiles.update();
    tiles.update();
    assert!(tiles.shows_layout());
    script.take_calls();

    let taken = OverscaledTileID::with_zxy(0, 1, 0, 0, 0);
    tiles.add_from_copy(taken);
    assert!(tiles.tile_at(&taken).is_renderable(), "it shows the copy's contents at once");
    assert!(tiles.tile_at(&taken).is_settled());
    assert!(script.take_calls().contains(&Call::TookCopy(taken)));
    let params = tiles.tile_params();
    tiles.with_at(&taken, |tile| tile.set_style(&red, params.pixel_ratio, params.subdivision_granularity));
    tiles.set_necessity(&taken, TileNecessity::Required);
    tiles.update();
    tiles.update();
    assert!(script.take_loads().is_empty(), "the tile that took its copy loads nothing");
    let shown = tiles.tile_at(&taken).shown.as_ref().expect("contents");
    assert!(!shown.world_copy, "it laid the copy's data out for itself");

    script.refuse_copies();
    let refused = OverscaledTileID::with_zxy(0, 2, 0, 0, 0);
    tiles.add_from_copy(refused);
    assert!(tiles.tile_at(&refused).is_renderable(), "it shows the copy's contents at once");
    assert!(!tiles.tile_at(&refused).is_settled(), "it has yet to load");
    assert!(!script.take_calls().contains(&Call::TookCopy(refused)));
    tiles.set_necessity(&refused, TileNecessity::Required);
    let loads = script.take_loads();
    assert_eq!(loads.iter().map(|load| load.tile).collect::<Vec<_>>(), [refused]);
}

/// The value that tile `id` of `tiles`, a value tile, shows, and the
/// identity of its delivery.
fn shown_value(tiles: &OneTile, id: &OverscaledTileID) -> (Arc<dyn std::any::Any + Send + Sync>, SimpleIdentity) {
    match tiles.tile_at(id).contents() {
        Some(Contents::Value(value)) => (Arc::clone(&value.value), value.id),
        _ => panic!("the tile shows no value"),
    }
}

/// The bytes of the value that `value`, a scripted value, holds.
fn value_bytes(value: &Arc<dyn std::any::Any + Send + Sync>) -> &Data {
    &value.downcast_ref::<ScriptedValue>().expect("a scripted value").0
}

/// A value tile shows each value its origin delivers at once, with no job,
/// as a new delivery that layers make their tile values of again, and lets
/// go of the value it showed before. An empty value tile draws nothing in
/// its place, and a missing one lets another zoom level's draw.
#[test]
fn a_value_tile_shows_each_delivery_at_once() {
    use TileOperation::{EndParse, LoadFromNetwork, StartParse};
    let test = Test::new();
    let manual = Manual::default();
    let store = tile_store(&test.pool, Box::new(manual.clone()));
    let id = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tile = OneTile::with_script(store, VALUE, &Script::default(), &[id], MapMode::Continuous);
    tile.set_necessity(&id, TileNecessity::Required);
    let [load] = <[_; 1]>::try_from(tile.script.take_loads()).ok().expect("one load");
    tile.actions();

    let first: Data = Arc::new(vec![1]);
    load.reply(Reply::Bytes(Arc::clone(&first)));
    tile.update();
    assert_eq!(tile.actions(), [LoadFromNetwork, StartParse, EndParse]);
    assert!(tile.tile().is_complete() && tile.tile().is_renderable());
    let (value, first_id) = shown_value(&tile, &id);
    assert!(Arc::ptr_eq(value_bytes(&value), &first), "the tile shows the value it was given");
    drop(value);

    let second: Data = Arc::new(vec![2]);
    load.reply(Reply::Bytes(Arc::clone(&second)));
    tile.update();
    assert_eq!(tile.actions(), [LoadFromNetwork, StartParse, EndParse]);
    let (value, second_id) = shown_value(&tile, &id);
    assert!(Arc::ptr_eq(value_bytes(&value), &second));
    assert_ne!(first_id, second_id, "each delivery is new to the layers");
    assert_eq!(Arc::strong_count(&first), 1, "the tile let go of the value before");
    drop(value);
    assert_eq!(manual.submitted(), 0, "a value has no job");

    load.reply(Reply::Empty);
    tile.update();
    assert!(matches!(tile.tile().contents(), Some(Contents::Empty)) && tile.tile().is_renderable());
    load.reply(Reply::Missing);
    tile.update();
    assert!(tile.tile().is_complete() && !tile.tile().is_renderable(), "another zoom level draws in its place");

    load.reply(Reply::Bytes(Arc::clone(&second)));
    tile.update();
    tile.drop_tile();
    assert_eq!(Arc::strong_count(&second), 1, "the value goes with its tile");
}

/// A value tile that starts from the same tile in another world copy shows
/// and takes that tile's value itself, which loads nothing.
#[test]
fn a_value_tiles_world_copy_shares_its_value() {
    let test = Test::new();
    let script = Script::default();
    let store = tile_store(&test.pool, Box::new(InlineWorkers));
    let loaded = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tiles = OneTile::with_script(store, VALUE, &script, &[loaded], MapMode::Continuous);
    tiles.set_necessity(&loaded, TileNecessity::Required);
    script.answer(|_| Reply::Bytes(Arc::new(vec![1])));
    tiles.update();
    script.take_calls();

    let copy = OverscaledTileID::with_zxy(0, 1, 0, 0, 0);
    tiles.add_from_copy(copy);
    assert!(script.take_calls().contains(&Call::TookCopy(copy)));
    assert!(tiles.tile_at(&copy).is_complete() && tiles.tile_at(&copy).is_renderable());
    assert!(Arc::ptr_eq(&shown_value(&tiles, &copy).0, &shown_value(&tiles, &loaded).0));
    tiles.set_necessity(&copy, TileNecessity::Required);
    assert!(script.take_loads().is_empty(), "the tile that took its copy loads nothing");
}
