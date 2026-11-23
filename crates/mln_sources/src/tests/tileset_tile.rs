//! Port-only: how a tileset source's tiles load: from the cache, then the
//! network while required; what each kind of response leaves them with; and
//! when they load again.

use std::sync::Arc;
use std::time::Duration;

use mln_map_types::{MapMode, TransformState};
use mln_render::source::{TileNecessity, TileOperation};
use mln_render::testing::{Event, OneSource, OriginTiles, View};
use mln_storage::response::Reason;
use mln_storage::{FileSource, Resource, ResourceError, Responder, Response, StoragePolicy};
use mln_style::layer_type::SourceKind;
use mln_style::source_type::SourceCommon;
use mln_style::{Layer, Source};
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::Timestamp;
use mln_util::geo::LatLng;
use mln_util::range::Range;
use mln_util::tileset::{PartialTileset, Tileset};
use parking_lot::Mutex;

use super::{TileTest, errors, ready, registry, view_state};
use crate::tileset::origin::{TilesetOrigin, VectorTileEncoding};
use crate::tileset::{RasterKind, TilesetKind, VectorKind};
use crate::vector_source;

/// A file source whose requests wait until the test answers them.
#[derive(Default)]
struct Held(Mutex<Vec<(Resource, Responder)>>);

impl Held {
    /// Answers the oldest request with `response`.
    fn respond(&self, response: Response) {
        let (_, responder) = self.0.lock().remove(0);
        responder.send(response);
    }

    fn pending(&self) -> usize {
        self.0.lock().len()
    }

    /// The resource of the oldest request.
    fn resource(&self) -> Resource {
        self.0.lock()[0].0.clone()
    }

    /// The requests of tile `url` so far, oldest first: each one's
    /// resource, and whether its requester stopped it.
    fn requests(&self, url: &str) -> Vec<(Resource, bool)> {
        let requests = self.0.lock();
        let of_tile = requests.iter().filter(|(resource, _)| resource.url == url);
        of_tile.map(|(resource, responder)| (resource.clone(), responder.is_cancelled())).collect()
    }
}

impl FileSource for Held {
    fn load(&self, resource: &Resource, responder: Responder) {
        self.0.lock().push((resource.clone(), responder));
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

/// `response`, expired: a tile that has it requests it again when required.
fn expired(mut response: Response) -> Response {
    response.expires = Some(Timestamp::from_secs(0));
    response
}

/// A tile cut from an ancestor that another tile brought fresh from the
/// network takes it from memory, without a request; with the ancestor's
/// data expired, it loads the ancestor again.
#[test]
fn a_cut_tile_takes_a_fresh_ancestor_from_memory() {
    let test = TileTest::new();
    let registry = registry();
    for (response, requests) in [(water(), 0), (expired(water()), 1)] {
        let file_source = Arc::new(Held::default());
        let file_source_dyn: Arc<dyn FileSource> = Arc::<Held>::clone(&file_source);
        let tileset = Tileset { tiles: vec!["tiles".to_owned()], zoom_range: Range::new(0, 0), ..Default::default() };
        let origin = TilesetOrigin::<VectorKind>::loaded(tileset, VectorTileEncoding::Mvt, Some(file_source_dyn));
        let (left, right) = (OverscaledTileID::from_zxy(1, 0, 0), OverscaledTileID::from_zxy(1, 1, 0));
        let mut tiles = OriginTiles::new(&test.thread_pool, origin, &[left, right], MapMode::Static);
        let mut layer = Layer::new(registry.style(), "circle", "water", "source").unwrap();
        layer.set_source_layer("water");
        tiles.set_layers(&registry, &[layer]).unwrap();
        tiles.set_necessity(&left, TileNecessity::Required);
        file_source.respond(response);
        tiles.update();
        tiles.update();
        assert!(tiles.is_complete(&left));

        tiles.set_necessity(&right, TileNecessity::Required);
        assert_eq!(file_source.pending(), requests);
        if requests == 0 {
            tiles.update();
            tiles.update();
            assert!(tiles.is_complete(&right));
        }
    }
}

/// Tile 0/0/0 of a tileset source of kind `K`, whose tiles (of `tileset`)
/// load through a new [`Held`], and that file source. A geometry tile lays
/// out a circle layer of the "water" source layer.
fn held_tile<K: TilesetKind>(test: &TileTest, tileset: Tileset) -> (OriginTiles, Arc<Held>) {
    let file_source = Arc::new(Held::default());
    let file_source_dyn: Arc<dyn FileSource> = Arc::<Held>::clone(&file_source);
    let origin = TilesetOrigin::<K>::loaded(tileset, VectorTileEncoding::Mvt, Some(file_source_dyn));
    let mut tiles = OriginTiles::new(&test.thread_pool, origin, &[tile()], MapMode::Continuous);
    if K::INFO.layer_kind == SourceKind::Geometry {
        let registry = registry();
        let mut layer = Layer::new(registry.style(), "circle", "water", "source").unwrap();
        layer.set_source_layer("water");
        tiles.set_layers(&registry, &[layer]).unwrap();
    }
    (tiles, file_source)
}

/// The tile that [`held_tile`] makes.
fn tile() -> OverscaledTileID {
    OverscaledTileID::from_zxy(0, 0, 0)
}

fn tiles_tileset() -> Tileset {
    Tileset { tiles: vec!["tiles".to_owned()], ..Default::default() }
}

/// Answers `tiles`' tile's request with `response` and lets it settle.
fn answer(tiles: &mut OriginTiles, file_source: &Held, response: Response) {
    file_source.respond(response);
    tiles.update();
    tiles.update();
}

/// A response without content (204, or no tile in an archive).
fn no_content() -> Response {
    let mut response = Response::new();
    response.no_content = true;
    response
}

/// A response that says the tileset has no tile here (404).
fn not_found() -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "not found")));
    response
}

/// An empty response leaves a tile empty, which draws; a 404 leaves a
/// vector tile empty but a raster tile missing, which a tile of another zoom
/// draws in place of; and a sparse tileset's empty tiles are missing.
#[test]
fn empty_and_missing_tiles_by_kind() {
    let test = TileTest::new();
    let sparse = Tileset { sparse: true, ..tiles_tileset() };
    let cases = [
        (SourceKind::Geometry, no_content(), tiles_tileset(), true),
        (SourceKind::Geometry, not_found(), tiles_tileset(), true),
        (SourceKind::Geometry, no_content(), sparse.clone(), false),
        (SourceKind::Raster, no_content(), tiles_tileset(), true),
        (SourceKind::Raster, not_found(), tiles_tileset(), false),
        (SourceKind::Raster, no_content(), sparse, false),
    ];
    for (i, (kind, response, tileset, renderable)) in cases.into_iter().enumerate() {
        let (mut tiles, file_source) = match kind {
            SourceKind::Geometry => held_tile::<VectorKind>(&test, tileset),
            _ => held_tile::<RasterKind>(&test, tileset),
        };
        tiles.set_necessity(&tile(), TileNecessity::Required);
        answer(&mut tiles, &file_source, response);
        assert!(tiles.is_complete(&tile()), "case {i}");
        assert_eq!(tiles.is_renderable(&tile()), renderable, "case {i}");
        assert_eq!(errors(&mut tiles), Vec::<String>::new(), "case {i}");
    }
}

/// A response that the server failed to give, with `message`.
fn failed(message: &str) -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::Other, message)));
    response
}

/// A "not modified" answer, whose data has expired again.
fn not_modified() -> Response {
    let mut response = expired(Response::new());
    response.not_modified = true;
    response
}

/// What `tiles` reported their tiles' loads did since the last call.
fn actions(tiles: &mut OriginTiles) -> Vec<TileOperation> {
    let events = tiles.take_events().into_iter();
    events
        .filter_map(|event| match event {
            Event::Action(op) => Some(op),
            Event::Changed | Event::Error { .. } => None,
        })
        .collect()
}

/// A failed load reports its error and asks again only as the engine
/// retries it.
#[test]
fn a_failed_load_is_requested_again_on_retry() {
    let test = TileTest::new();
    let (mut tiles, file_source) = held_tile::<VectorKind>(&test, tiles_tileset());
    tiles.set_necessity(&tile(), TileNecessity::Required);
    answer(&mut tiles, &file_source, failed("failed"));
    assert_eq!(errors(&mut tiles), ["failed"]);
    assert_eq!(file_source.pending(), 0);

    tiles.retry_failed();
    assert_eq!(file_source.pending(), 1);
    answer(&mut tiles, &file_source, water());
    assert!(tiles.is_complete(&tile()));
    assert_ne!(tiles.features(&tile(), "water").unwrap(), Vec::new());
}

/// A tile required again loads again only if its data isn't fresh from the
/// network: data without an expiry, or whose expiry is to come, stays fresh.
/// The reload asks with the data's validators, and a "not modified" answer
/// keeps the data.
#[test]
fn a_tile_required_again_reloads_only_stale_data() {
    let test = TileTest::new();
    let mut later = water();
    later.expires = Some(Timestamp::from_secs(i64::MAX));
    for (response, reloads) in [(water(), false), (later, false), (expired(water()), true)] {
        let (mut tiles, file_source) = held_tile::<VectorKind>(&test, tiles_tileset());
        tiles.set_necessity(&tile(), TileNecessity::Required);
        answer(&mut tiles, &file_source, response);
        assert!(tiles.is_complete(&tile()));

        tiles.set_necessity(&tile(), TileNecessity::Optional);
        tiles.set_necessity(&tile(), TileNecessity::Required);
        assert_eq!(file_source.pending(), usize::from(reloads));
        if reloads {
            assert_eq!(file_source.resource().prior_etag.as_deref(), Some("a"));
            let mut not_modified = Response::new();
            not_modified.not_modified = true;
            answer(&mut tiles, &file_source, not_modified);
            assert!(tiles.is_complete(&tile()));
        }
        assert_ne!(tiles.features(&tile(), "water").unwrap(), Vec::new());
    }
}

/// A reload that the server answers "not modified" keeps the data the tile
/// has: the tile stays complete while it reloads, and shows no new contents.
/// A revalidation that fails, retried and answered "not modified", completes
/// the tile without new contents too.
#[test]
fn a_not_modified_reload_keeps_the_data() {
    use TileOperation::{EndParse, LoadFromNetwork, RequestedFromNetwork};

    let test = TileTest::new();
    let (mut tiles, file_source) = held_tile::<VectorKind>(&test, tiles_tileset());
    tiles.set_necessity(&tile(), TileNecessity::Required);
    answer(&mut tiles, &file_source, expired(water()));
    assert!(tiles.is_complete(&tile()));
    let shown = tiles.features(&tile(), "water").unwrap();
    assert_ne!(shown, Vec::new());
    tiles.take_events();

    // Required again, the tile revalidates its expired data.
    tiles.set_necessity(&tile(), TileNecessity::Optional);
    tiles.set_necessity(&tile(), TileNecessity::Required);
    assert_eq!(file_source.pending(), 1);
    assert!(tiles.is_complete(&tile()), "a reload keeps the tile complete");
    answer(&mut tiles, &file_source, not_modified());
    assert_eq!(actions(&mut tiles), [RequestedFromNetwork, LoadFromNetwork], "no new contents");
    assert!(tiles.is_complete(&tile()));

    // The answer is all the retry brings, and the tile completes.
    tiles.set_necessity(&tile(), TileNecessity::Optional);
    tiles.set_necessity(&tile(), TileNecessity::Required);
    answer(&mut tiles, &file_source, failed("offline"));
    tiles.retry_failed();
    assert!(!tiles.is_complete(&tile()), "the tile waits for the retry");
    answer(&mut tiles, &file_source, not_modified());
    assert!(tiles.is_complete(&tile()));
    assert!(!actions(&mut tiles).contains(&EndParse), "no new contents");
    assert_eq!(tiles.features(&tile(), "water").unwrap(), shown);
}

/// A tile whose data failed to lay out loads it again when required again,
/// though the data has no expiry.
#[test]
fn a_tile_whose_data_fails_to_lay_out_is_requested_again() {
    let test = TileTest::new();
    let (mut tiles, file_source) = held_tile::<VectorKind>(&test, tiles_tileset());
    tiles.set_necessity(&tile(), TileNecessity::Required);
    let mut response = Response::new();
    response.data = Some(Arc::new(b"not a tile".to_vec()));
    answer(&mut tiles, &file_source, response);
    assert_eq!(errors(&mut tiles).len(), 1);

    tiles.set_necessity(&tile(), TileNecessity::Optional);
    tiles.set_necessity(&tile(), TileNecessity::Required);
    assert_eq!(file_source.pending(), 1);
}

/// A tile's requests start in the tile's place among the requests that
/// wait, after those of anything else, which leave theirs at zero.
#[test]
fn a_request_takes_the_tiles_order() {
    let test = TileTest::new();
    let (mut tiles, file_source) = held_tile::<VectorKind>(&test, tiles_tileset());
    tiles.set_necessity(&tile(), TileNecessity::Required);
    assert!(file_source.resource().order.get() > 0);
}

/// A response that arrives as the tile stops its request, on becoming
/// optional, changes nothing: the tile stays without data, and asks again
/// when required again.
#[test]
fn a_response_to_a_stopped_request_changes_nothing() {
    let test = TileTest::new();
    let (mut tiles, file_source) = held_tile::<VectorKind>(&test, tiles_tileset());
    tiles.set_necessity(&tile(), TileNecessity::Required);
    // The response waits for the store's next update.
    file_source.respond(water());
    tiles.set_necessity(&tile(), TileNecessity::Optional);
    tiles.update();
    tiles.update();
    assert!(!tiles.is_renderable(&tile()));
    assert!(!actions(&mut tiles).contains(&TileOperation::LoadFromNetwork));

    tiles.set_necessity(&tile(), TileNecessity::Required);
    assert_eq!(file_source.pending(), 1);
}

/// A vector source "source" whose tiles are at `{z}/{x}/{y}`, the render
/// source of it, drawn by a circle layer, and a file source that holds its
/// tiles' requests.
fn held_source(test: &TileTest) -> (Source, OneSource, Arc<Held>) {
    let tileset = PartialTileset { tiles: Some(vec!["{z}/{x}/{y}".to_owned()]), ..Default::default() };
    let source = ready(vector_source("source", None, tileset));
    let registry = registry();
    let mut layer = Layer::new(registry.style(), "circle", "water", "source").unwrap();
    layer.set_source_layer("water");
    let render_source = OneSource::new(&registry, &source, &[layer], &test.thread_pool).unwrap();
    (source, render_source, Arc::new(Held::default()))
}

/// What a [`held_source`] updates for: `state`, its tiles loading through
/// `file_source`.
fn held_view<'a>(state: &'a TransformState, file_source: &Arc<Held>) -> View<'a> {
    let file_source: Arc<dyn FileSource> = Arc::<Held>::clone(file_source);
    View { state, mode: MapMode::Continuous, file_source: Some(file_source), needs_relayout: false }
}

/// `source` after `change` to its common members.
fn with_common(source: &Source, change: fn(&mut SourceCommon)) -> Source {
    let mut common = *source.common();
    change(&mut common);
    source.with_common(common)
}

/// What a network request asks of `resource`'s data's freshness: its
/// minimum update interval, and whether it stays out of the offline cache.
fn freshness_of(resource: &Resource) -> (Duration, bool) {
    (resource.minimum_update_interval, resource.storage_policy == StoragePolicy::Volatile)
}

/// What `common` asks of its tiles' network requests, as [`freshness_of`].
fn freshness_in(common: &SourceCommon) -> (Duration, bool) {
    (common.minimum_tile_update_interval, common.volatile)
}

/// A required tile's network request is made again, with the source's new
/// minimum update interval or volatility, as the next update retains the
/// tile; an update that changes neither makes no request.
#[test]
fn a_network_request_restarts_when_the_update_interval_changes() {
    let changes: [fn(&mut SourceCommon); 2] =
        [|common| common.minimum_tile_update_interval = Duration::from_secs(1), |common| common.volatile = true];
    for (i, change) in changes.into_iter().enumerate() {
        let test = TileTest::new();
        let (source, mut tiles, file_source) = held_source(&test);
        let state = view_state(0.0);
        let view = held_view(&state, &file_source);
        tiles.update(&source, &view);
        let requests = file_source.requests("0/0/0");
        assert_eq!(requests.len(), 1, "case {i}");
        assert_eq!(freshness_of(&requests[0].0), freshness_in(source.common()), "case {i}");

        let source = with_common(&source, change);
        tiles.update(&source, &view);
        let requests = file_source.requests("0/0/0");
        assert_eq!(requests.len(), 2, "case {i}");
        assert!(requests[0].1, "case {i}: the request before stops");
        assert!(!requests[1].1, "case {i}");
        assert_eq!(freshness_of(&requests[1].0), freshness_in(source.common()), "case {i}");

        tiles.update(&source, &view);
        assert_eq!(file_source.requests("0/0/0").len(), 2, "case {i}");
    }
}

/// So is the request of a tile that left the view while it loaded, and went
/// on loading in the cache, while the update interval changed: as the view
/// comes back and retains it.
#[test]
fn a_network_request_restarts_when_the_update_interval_changes_while_it_loads_in_the_cache() {
    let test = TileTest::new();
    let (source, mut tiles, file_source) = held_source(&test);
    let here = view_state(2.0);
    // A tile to the east: tile 2/1/1 leaves the view, next to its tiles.
    let mut east = TransformState::clone(&here);
    east.set_lat_lng_zoom(&LatLng::new(0.0, 90.0).unwrap(), 2.0);
    tiles.update(&source, &held_view(&here, &file_source));
    assert_eq!(file_source.requests("2/1/1").len(), 1);

    let source = with_common(&source, |common| common.minimum_tile_update_interval = Duration::from_secs(1));
    tiles.update(&source, &held_view(&east, &file_source));
    let requests = file_source.requests("2/1/1");
    assert_eq!(requests.len(), 1, "the cache doesn't retain the tile");
    assert!(!requests[0].1, "the tile goes on loading in the cache");

    tiles.update(&source, &held_view(&here, &file_source));
    let requests = file_source.requests("2/1/1");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].1, "the request before stops");
    assert_eq!(freshness_of(&requests[1].0), freshness_in(source.common()));
}
