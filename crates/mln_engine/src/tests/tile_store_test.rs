// Port-only: the tile store keeps a tile's data, work and contents apart.
// Data events name their load and never the layout's generation; replies
// apply only to the generation that asked; contents keep the data they were
// built from while newer data lays out; and a tile that leaves and comes
// back, or a source replaced, never takes what was meant for the one before.

use std::sync::Arc;

use geo_types::{Geometry, Point};
use mln_style::Source;
use mln_style::feature::{Feature, PropertyMap};
use mln_style::layer::SourceKind;
use mln_style::source::SourceData;
use mln_style::sources::tileset::{TilesetSourceOptions, VectorSourceType};
use mln_style::tileset::{PartialTileset, Tileset};
use parking_lot::Mutex;
use serde_json::json;

use super::tile_jobs_test::{Manual, icons, layer, points};
use crate::exec::pool::TaskGroup;
use crate::layer::layer_drawing::ChangedBuckets;
use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::map::MapMode;
use crate::query::SourceQueryOptions;
use crate::storage::chrono::Timestamp;
use crate::storage::response::{Reason, ResourceError};
use crate::storage::{FileSource, Resource, Responder, Response};
use crate::tests::tile::{dem_png, png};
use crate::tests::{OneTile, map_settings, origin_of, style, tile_store, tileset_tile};
use crate::tile::dem_data::DEMData;
use crate::tile::{OverscaledTileID, TileData};
use crate::tile_store::contents::Contents;
use crate::tile_store::inbox::TileEvent;
use crate::tile_store::pipeline::LayoutWork;
use crate::tile_store::tile::{TileNecessity, TileOperation};
use crate::worker::job::{DataInput, Job, Run, TileKey};
use crate::worker::tile_workers::{InlineWorkers, TileWorkers};

/// A file source whose requests wait until the test answers them.
#[derive(Default)]
struct Held(Mutex<Vec<Responder>>);

impl Held {
    /// Answers the oldest request with `response`.
    fn respond(&self, response: Response) {
        let responder = self.0.lock().remove(0);
        responder.send(response);
    }

    fn pending(&self) -> usize {
        self.0.lock().len()
    }
}

impl FileSource for Held {
    fn load(&self, _resource: &Resource, responder: Responder) {
        self.0.lock().push(responder);
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

/// A response with the bytes of a vector tile with a "water" layer.
fn water() -> Response {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("testdata/test/fixtures/map/issue12432/0-0-0.mvt");
    let mut response = Response::new();
    response.data = Some(Arc::new(std::fs::read(path).expect("the tile")));
    response.etag = Some("a".to_owned());
    response
}

/// `water()` with other bytes: an unknown field the tile's reader skips.
fn water_changed() -> Response {
    let mut response = water();
    let mut bytes = response.data.as_deref().unwrap().clone();
    // Field 15, a varint of 1.
    bytes.extend_from_slice(&[0x78, 0x01]);
    response.data = Some(Arc::new(bytes));
    response
}

/// `response`, expired: a tile that has it requests it again when required.
fn expired(mut response: Response) -> Response {
    response.caching.expires = Some(Timestamp::from_secs(0));
    response
}

/// A `not_modified` answer whose data has expired again.
fn not_modified() -> Response {
    let mut response = expired(Response::new());
    response.not_modified = true;
    response
}

fn water_layer(id: &str, color: &str) -> Arc<dyn ErasedEvaluatedLayer> {
    layer(json!({"id": id, "type": "fill", "source-layer": "water", "paint": {"fill-color": color}}))
}

/// Features with ids `ids`, at the middle of the tile.
fn features(ids: &[u64]) -> TileData {
    let features = ids
        .iter()
        .map(|id| Feature {
            geometry: Some(Geometry::Point(Point::new(2048i16, 2048))),
            properties: Arc::new(PropertyMap::new()),
            id: mln_style::feature::Identifier::Uint(*id),
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

    /// Tile 0/0/0 of a vector tileset whose tiles `file_source` answers,
    /// whose jobs run on `workers`.
    fn tileset_tile(&self, file_source: &Arc<Held>, workers: Box<dyn TileWorkers>) -> OneTile {
        let file_source: Arc<dyn FileSource> = Arc::<Held>::clone(file_source);
        let store = tile_store(&self.pool, workers);
        let source = vector_source(Tileset { tiles: vec!["tiles".to_owned()], ..Default::default() });
        let id = OverscaledTileID::from_zxy(0, 0, 0);
        OneTile::with_origin(store, &source, Some(file_source), &[id], MapMode::Static)
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

/// A vector source of `tileset`, which it lists.
fn vector_source(tileset: Tileset) -> Source {
    let options = TilesetSourceOptions {
        url: None,
        tileset: PartialTileset::default(),
        tile_size: mln_style::constants::TILE_SIZE_I,
    };
    Source::new("source", SourceData::<VectorSourceType>::with_description(options, Arc::new(tileset)))
}

/// Whether `tile`'s tileset load waits for a network-only request, or (with
/// `false`) for no request at all.
fn requesting_network(tile: &OneTile, pending: bool) -> bool {
    let tileset = tileset_tile(tile.tile());
    tileset.is_requesting() == pending
}

/// Begins a load of `tile`, as a GeoJSON source's `setData` does, and
/// returns it.
fn begin_load(tile: &mut OneTile) -> u64 {
    tile.with(|tile| {
        let (info, tile) = tile.parts();
        let load = info.next();
        tile.load.current = Some(load);
        load
    })
}

/// A reply to load `load` of tile `id` of source `key` that gives the tile
/// `data`.
fn data_reply(key: crate::worker::job::SourceKey, id: OverscaledTileID, load: u64, data: TileData) -> TileEvent {
    TileEvent::Load { tile: TileKey { source: key, id }, load, reply: Box::new(Some(data)) }
}

/// The style of a layout job.
fn job_style(run: &Run) -> &Arc<crate::worker::job::SourceStyle> {
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
    let file_source = Arc::new(Held::default());
    let manual = Manual::default();
    let mut tile = test.tileset_tile(&file_source, Box::new(manual.clone()));
    let id = tile.id;
    tile.set_style(&style(&[water_layer("water", "red")]));
    tile.set_necessity(&id, TileNecessity::Required);
    assert_eq!(file_source.pending(), 1);

    let edited = style(&[water_layer("water", "blue")]);
    tile.set_style(&edited);
    file_source.respond(water());
    tile.update();
    assert_eq!(manual.submitted(), 1, "the data applied");
    let run = manual.take();
    assert!(Arc::ptr_eq(job_style(&run), &edited), "with the newest style");
    manual.reply(InlineWorkers::run(run));
    tile.update();
    assert!(tile.shows_layout() && tile.tile().is_complete());
}

/// A reload that the server answers `not_modified` keeps the data the tile
/// has: the tile neither changes nor lays out again.
#[test]
fn a_not_modified_reload_keeps_the_data() {
    let test = Test::new();
    let file_source = Arc::new(Held::default());
    let manual = Manual::default();
    let mut tile = test.tileset_tile(&file_source, Box::new(manual.clone()));
    let id = tile.id;
    tile.set_style(&style(&[water_layer("water", "red")]));
    tile.set_necessity(&id, TileNecessity::Required);
    file_source.respond(expired(water()));
    tile.update();
    manual.run();
    tile.update();
    assert!(tile.tile().is_complete());
    let key = tile.layout().data.as_ref().expect("data").key;
    let shown_layout = |tile: &OneTile| {
        let contents = tile.tile().contents().and_then(Contents::geometry).expect("contents");
        Arc::clone(&contents.result.feature_index)
    };
    let shown = shown_layout(&tile);

    // Required again, the tile revalidates its expired data.
    tile.set_necessity(&id, TileNecessity::Optional);
    tile.set_necessity(&id, TileNecessity::Required);
    assert_eq!(file_source.pending(), 1);
    assert!(requesting_network(&tile, true));
    assert!(tile.tile().is_complete(), "a reload keeps the tile complete");
    file_source.respond(not_modified());
    tile.update();
    assert!(requesting_network(&tile, true), "the request stays open for the next revalidation");
    assert_eq!(tile.layout().data.as_ref().expect("data").key, key, "the same data");
    assert_eq!(manual.submitted(), 1, "nothing to lay out again");
    assert!(Arc::ptr_eq(&shown_layout(&tile), &shown), "the same contents");
    assert!(tile.tile().is_complete());

    // A revalidation that fails, retried and answered `not_modified`: the
    // answer is all the retry brings, and the tile completes.
    tile.set_necessity(&id, TileNecessity::Optional);
    tile.set_necessity(&id, TileNecessity::Required);
    let mut failed = Response::new();
    failed.error = Some(Box::new(ResourceError::new(Reason::Connection, "offline")));
    file_source.respond(failed);
    tile.update();
    tile.store.retry_failed();
    assert!(!tile.tile().is_complete(), "the tile waits for the retry");
    file_source.respond(not_modified());
    tile.update();
    assert_eq!(manual.submitted(), 1, "nothing to lay out again");
    assert!(tile.tile().is_complete());
}

/// A tile whose data failed to lay out requests it again when required
/// again, though the data has no expiry.
#[test]
fn failed_data_is_requested_again() {
    let test = Test::new();
    let file_source = Arc::new(Held::default());
    let manual = Manual::default();
    let mut tile = test.tileset_tile(&file_source, Box::new(manual.clone()));
    let id = tile.id;
    tile.set_style(&style(&[water_layer("water", "red")]));
    tile.set_necessity(&id, TileNecessity::Required);
    let mut response = Response::new();
    response.data = Some(Arc::new(b"not a tile".to_vec()));
    file_source.respond(response);
    tile.update();
    manual.run();
    tile.update();
    assert!(tile.actions().contains(&TileOperation::Error));

    tile.set_necessity(&id, TileNecessity::Optional);
    tile.set_necessity(&id, TileNecessity::Required);
    assert_eq!(file_source.pending(), 1);
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
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "circle"}))]));
    tile.set_data(Some(features(&[1])));
    tile.update();
    let old = manual.take();

    tile.drop_tile();
    assert!(old.superseded.load(std::sync::atomic::Ordering::Relaxed), "a worker may skip it");
    tile.add(id);
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "circle"}))]));
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
    assert_eq!(shown[0].id, mln_style::feature::Identifier::Uint(2));
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
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "circle"}))]));
    tile.set_data(Some(features(&[1])));
    tile.update();
    manual.run();
    tile.update();
    assert!(tile.shows_layout());

    tile.set_data(Some(features(&[2])));
    tile.update();
    assert_eq!(manual.submitted(), 2, "the new data lays out");
    let key = tile.key;
    let query = |tile: &OneTile| -> Vec<mln_style::feature::Identifier> {
        let source = tile.store.source(key).expect("the source");
        let options = SourceQueryOptions { source_layers: Some(vec![String::new()]), filter: None };
        source
            .query_source_features(&options, None, &crate::geo::world::WorldMapping::Mercator)
            .unwrap()
            .into_iter()
            .map(|feature| feature.id)
            .collect()
    };
    assert_eq!(query(&tile), [mln_style::feature::Identifier::Uint(1)], "the shown contents' data");

    // A feature state edit reaches the shown contents, over their data.
    let mut state = mln_style::feature::FeatureState::new();
    state.insert("hover".to_owned(), mln_style::feature::Value::Bool(true));
    let (source, _) = tile.store.source_mut(key).expect("the source");
    source.info.feature_state.update_state(None, "1", &state);
    source.coalesce_feature_state(&mut ChangedBuckets::default());
    let version = source.info.feature_state.version();
    let shown_version = |tile: &OneTile| tile.tile().shown.as_ref().expect("contents").feature_state_version;
    // Not rendered, so the coalesce left it.
    assert_eq!(shown_version(&tile), 0);
    tile.with(|tile| {
        let (info, tile) = tile.parts();
        info.feature_state.coalesce_changes(tile.shown.as_mut(), &mut ChangedBuckets::default());
    });
    assert_eq!(shown_version(&tile), version);

    manual.run();
    tile.update();
    assert_eq!(query(&tile), [mln_style::feature::Identifier::Uint(2)]);
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
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "circle"}))]));
    // A load under way, as a GeoJSON source's `setData` starts.
    let old_load = begin_load(&mut tile);
    tile.drop_tile();
    tile.add(id);
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "circle"}))]));
    let new_load = begin_load(&mut tile);

    let inbox = tile.store.inbox().clone();
    inbox.send(data_reply(key, id, old_load, features(&[1])));
    tile.update();
    assert!(tile.layout().data.as_ref().is_none(), "the old tile's data goes nowhere");
    inbox.send(data_reply(key, id, new_load, features(&[2])));
    inbox.send(data_reply(key, id, old_load, features(&[3])));
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
    assert_eq!(features[0].id, mln_style::feature::Identifier::Uint(2));
}

/// A source removed while its tiles load and lay out, and one of the same
/// id added in its place, as when a style is replaced mid-load: what
/// arrives for the old source's tiles reaches none of the new one's.
#[test]
fn style_replaced_mid_load() {
    let test = Test::new();
    let file_source = Arc::new(Held::default());
    let manual = Manual::default();
    let mut tile = test.tileset_tile(&file_source, Box::new(manual.clone()));
    let id = tile.id;
    let layers = [water_layer("water", "red")];
    tile.set_style(&style(&layers));
    tile.set_necessity(&id, TileNecessity::Required);
    file_source.respond(expired(water()));
    tile.update();
    let old_job = manual.take();
    // A reload under way.
    tile.set_necessity(&id, TileNecessity::Optional);
    tile.set_necessity(&id, TileNecessity::Required);
    let reload = file_source.0.lock().remove(0);

    // The new style's source of the same id.
    let params = tile.tile_params();
    let old_key = tile.key;
    tile.store.remove_source(old_key);
    assert!(old_job.superseded.load(std::sync::atomic::Ordering::Relaxed));
    assert!(reload.is_cancelled(), "the old tile's request goes with it");
    let new_source = vector_source(Tileset { tiles: vec!["tiles".to_owned()], ..Default::default() });
    let new_key = tile.store.add_source("source".to_owned(), origin_of(&new_source));
    assert_ne!(new_key, old_key, "keys aren't reused");
    let (source, ctx) = tile.store.source_mut(new_key).expect("the source");
    let file_source_dyn: Arc<dyn FileSource> = Arc::<Held>::clone(&file_source);
    source.update_origin(&new_source, ctx, &map_settings(MapMode::Static, Some(file_source_dyn)), false);
    source.add_tile(id, params, false, ctx).expect("a tile");
    tile.key = new_key;
    tile.set_style(&style(&layers));

    // The old job's reply, and an old response sent before the cancel.
    manual.reply(InlineWorkers::run(old_job));
    let old_response = TileEvent::Load { tile: TileKey { source: old_key, id }, load: 1, reply: Box::new(water()) };
    tile.store.inbox().send(old_response);
    tile.update();
    assert!(tile.store.source(old_key).is_none());
    assert!(
        tile.tile().contents().is_none() && tile.layout().data.as_ref().is_none() && requesting_network(&tile, false)
    );
    assert_eq!(manual.submitted(), 1, "nothing for the new tile yet");

    tile.set_necessity(&id, TileNecessity::Required);
    file_source.respond(water());
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
fn dems<'a>(tiles: &'a OneTile, left: &OverscaledTileID, right: &OverscaledTileID) -> [&'a DEMData; 2] {
    [left, right].map(|id| match tiles.tile_at(id).contents() {
        Some(Contents::Dem(dem)) => dem.dem(),
        _ => panic!("a DEM"),
    })
}

/// Asserts that each of the DEMs of `left` and `right` has the other's edge
/// as its border.
fn assert_borders_match(tiles: &OneTile, left: &OverscaledTileID, right: &OverscaledTileID) {
    let [left_dem, right_dem] = dems(tiles, left, right);
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
    let edge = |tiles: &OneTile| -> Vec<f32> {
        let [_, right_dem] = dems(tiles, &left, &right);
        (0..16).map(|y| right_dem.get(0, y)).collect()
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
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "circle"}))]));
    let load = begin_load(&mut tile);
    let inbox = tile.store.inbox().clone();
    // From another thread, as a GeoJSON source's tiles cut on a pool answer.
    std::thread::spawn(move || inbox.send(data_reply(key, id, load, features(&[1])))).join().unwrap();
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
    let icon = |id: &str| layer(json!({"id": id, "type": "symbol", "layout": {"icon-image": id}}));
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

/// A required tile's network request stays open, and the tile takes each
/// answer the file source gives it: after errors, of which a streak fails
/// the tile once, its retry's data; when the data expires, a revalidation's
/// `not_modified`, which keeps the data, or new data, which lays out again.
#[test]
fn a_tile_takes_every_answer_to_its_open_request() {
    use TileOperation::{EndParse, Error, LoadFromNetwork, RequestedFromNetwork, StartParse};

    let test = Test::new();
    let file_source = Arc::new(Held::default());
    let mut tile = test.tileset_tile(&file_source, Box::new(InlineWorkers));
    let id = tile.id;
    tile.set_style(&style(&[water_layer("water", "red")]));
    tile.set_necessity(&id, TileNecessity::Required);
    let failed = || {
        let mut failed = Response::new();
        failed.error = Some(Box::new(ResourceError::new(Reason::Connection, "offline")));
        failed
    };
    let responder = file_source.0.lock().remove(0);
    responder.send(failed());
    responder.send(failed());
    tile.update();
    assert_eq!(tile.actions(), [StartParse, RequestedFromNetwork, Error], "one error for the streak");
    assert!(tile.tile().is_complete(), "a failed tile has done what it can for now");

    // The retry's answer.
    responder.send(water());
    tile.update();
    tile.update();
    assert_eq!(tile.actions(), [LoadFromNetwork, StartParse, EndParse]);
    assert!(tile.shows_layout() && tile.tile().is_complete());

    // Revalidations: unchanged, failed, then new data.
    responder.send(not_modified());
    tile.update();
    assert_eq!(tile.actions(), [LoadFromNetwork]);
    responder.send(failed());
    tile.update();
    assert_eq!(tile.actions(), [Error]);
    assert!(tile.shows_layout(), "a failed revalidation keeps the data");
    responder.send(water_changed());
    tile.update();
    tile.update();
    assert_eq!(tile.actions(), [LoadFromNetwork, StartParse, EndParse]);
    assert!(tile.shows_layout() && tile.tile().is_complete());

    assert!(!responder.is_cancelled(), "the request stays open");
    assert_eq!(file_source.pending(), 0, "no new request");
    tile.set_necessity(&id, TileNecessity::Optional);
    assert!(responder.is_cancelled(), "an optional tile drops it");
}

/// A tile that becomes optional while its failed request waits for the file
/// source's retry cancels it, and requests anew when required again.
#[test]
fn an_optional_tile_stops_retrying() {
    let test = Test::new();
    let file_source = Arc::new(Held::default());
    let mut tile = test.tileset_tile(&file_source, Box::new(InlineWorkers));
    let id = tile.id;
    tile.set_style(&style(&[water_layer("water", "red")]));
    tile.set_necessity(&id, TileNecessity::Required);
    let responder = file_source.0.lock().remove(0);
    let mut failed = Response::new();
    failed.error = Some(Box::new(ResourceError::new(Reason::Connection, "offline")));
    responder.send(failed);
    tile.update();

    tile.set_necessity(&id, TileNecessity::Optional);
    assert!(responder.is_cancelled());
    tile.set_necessity(&id, TileNecessity::Required);
    assert_eq!(file_source.pending(), 1, "a new request");
}

/// A required tile whose load failed starts parsing again when it is
/// retried: its parse events say `StartParse` once its new request is made,
/// and `EndParse` once its layout shows.
#[test]
fn a_retried_load_starts_parsing_again() {
    use TileOperation::{EndParse, Error, LoadFromNetwork, RequestedFromNetwork, StartParse};

    let test = Test::new();
    let file_source = Arc::new(Held::default());
    let mut tile = test.tileset_tile(&file_source, Box::new(InlineWorkers));
    let id = tile.id;
    tile.set_style(&style(&[water_layer("water", "red")]));
    tile.set_necessity(&id, TileNecessity::Required);
    let mut failed = Response::new();
    failed.error = Some(Box::new(ResourceError::new(Reason::Connection, "offline")));
    file_source.respond(failed);
    tile.update();
    assert_eq!(tile.actions(), [StartParse, RequestedFromNetwork, Error]);
    assert!(tile.tile().is_complete() && !tile.tile().is_pending());

    tile.store.retry_failed();
    assert_eq!(tile.actions(), [RequestedFromNetwork, StartParse]);
    file_source.respond(water());
    tile.update();
    tile.update();
    assert_eq!(tile.actions(), [LoadFromNetwork, EndParse]);
    assert!(tile.shows_layout() && tile.tile().is_complete());
}

/// A tile laid out without the glyphs of a range that failed lays out again
/// with them once another tile's wait loads the range, without a retry.
#[test]
fn a_tile_lays_out_again_once_a_failed_glyph_range_loads() {
    let test = Test::new();
    let file_source = Arc::new(Held::default());
    let store = tile_store(&test.pool, Box::new(InlineWorkers));
    let first = OverscaledTileID::from_zxy(1, 0, 0);
    let second = OverscaledTileID::from_zxy(1, 1, 0);
    let mut tiles = OneTile::new(store, SourceKind::Geometry, first, MapMode::Continuous);
    tiles.add(second);
    let glyph_source: Arc<dyn FileSource> = Arc::<Held>::clone(&file_source);
    tiles.store.set_file_source(Some(glyph_source));
    tiles.store.glyphs().set_url("test://{fontstack}/{range}.pbf");
    let labels = style(&[layer(json!({"id": "labels", "type": "symbol", "layout": {"text-field": "a"}}))]);
    let has_glyph = |tiles: &OneTile, id: &OverscaledTileID| {
        let contents = tiles.tile_at(id).contents().and_then(Contents::geometry).expect("a layout");
        let glyphs = &contents.answer.resolved.glyphs;
        glyphs
            .values()
            .flat_map(|positions| positions.iter())
            .any(|(id, position)| id.char() == Some('a') && position.is_some())
    };

    tiles.set_style(&labels);
    tiles.set_data(Some(points()));
    tiles.update();
    tiles.update();
    assert_eq!(file_source.pending(), 1, "the glyph range's request");
    let mut failed = Response::new();
    failed.error = Some(Box::new(ResourceError::new(Reason::Other, "offline")));
    file_source.respond(failed);
    tiles.settle(&[first]);
    assert!(!has_glyph(&tiles, &first), "laid out without the glyph");

    tiles.with_at(&second, |tile| {
        tile.set_style(&labels, 1.0, mln_style::subdivision_granularity::SubdivisionGranularitySetting::none());
        tile.take_data(Some(points()), false);
    });
    tiles.update();
    tiles.update();
    assert_eq!(file_source.pending(), 1, "the range's request again");
    let mut glyphs = Response::new();
    glyphs.data = Some(Arc::new(crate::tests::text::read_file("test/fixtures/resources/glyphs.pbf")));
    file_source.respond(glyphs);
    tiles.settle(&[first, second]);
    assert!(has_glyph(&tiles, &second));
    assert!(has_glyph(&tiles, &first), "laid out again with the glyph");
}
