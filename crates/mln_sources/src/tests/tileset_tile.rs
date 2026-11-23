//! Port-only: how a tileset source's tiles load: from the cache, then the
//! network while required; what each kind of response leaves them with; and
//! when they load again.

use std::sync::Arc;
use std::time::Duration;

use mln_map_types::{MapMode, TransformState};
use mln_render::source::{TileNecessity, TileOperation};
use mln_render::testing::{Event, OneSource, OriginTiles, View};
use mln_storage::resource::{Priority, TileFormat};
use mln_storage::response::Reason;
use mln_storage::{FileSource, LoadingMethod, Resource, ResourceError, Responder, Response, StoragePolicy};
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
use crate::tileset::{RasterDemKind, RasterKind, TilesetKind, VectorKind};
use crate::vector_source;

/// A file source whose requests wait until the test answers them, without
/// a cache, or with one ([`Held::with_cache`]).
#[derive(Default)]
struct Held {
    requests: Mutex<Vec<(Resource, Responder)>>,
    cache: bool,
}

impl Held {
    /// A file source with a cache, which answers cache-only requests.
    fn with_cache() -> Self {
        Held { cache: true, ..Held::default() }
    }

    /// Answers the oldest request with `response`; returns what it asked
    /// for.
    fn respond(&self, response: Response) -> Resource {
        let (resource, responder) = self.requests.lock().remove(0);
        responder.send(response);
        resource
    }

    /// Takes the oldest request's responder, through which the test answers
    /// it as often as it likes.
    fn take(&self) -> Responder {
        self.requests.lock().remove(0).1
    }

    fn pending(&self) -> usize {
        self.requests.lock().len()
    }

    /// The resource of the oldest request.
    fn resource(&self) -> Resource {
        self.requests.lock()[0].0.clone()
    }

    /// The requests of tile `url` so far, oldest first: each one's
    /// resource, and whether its requester stopped it.
    fn requests(&self, url: &str) -> Vec<(Resource, bool)> {
        let requests = self.requests.lock();
        let of_tile = requests.iter().filter(|(resource, _)| resource.url == url);
        of_tile.map(|(resource, responder)| (resource.clone(), responder.is_cancelled())).collect()
    }
}

impl FileSource for Held {
    fn load(&self, resource: &Resource, responder: Responder) {
        self.requests.lock().push((resource.clone(), responder));
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }

    fn supports_cache_only(&self) -> bool {
        self.cache
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
    (held_tile_from::<K>(test, tileset, VectorTileEncoding::Mvt, &file_source), file_source)
}

/// Tile 0/0/0 of a tileset source of kind `K`, whose tiles (of `tileset`,
/// in `encoding`) load through `file_source`, as [`held_tile`].
fn held_tile_from<K: TilesetKind>(
    test: &TileTest,
    tileset: Tileset,
    encoding: VectorTileEncoding,
    file_source: &Arc<Held>,
) -> OriginTiles {
    let file_source_dyn: Arc<dyn FileSource> = Arc::<Held>::clone(file_source);
    let origin = TilesetOrigin::<K>::loaded(tileset, encoding, Some(file_source_dyn));
    let mut tiles = OriginTiles::new(&test.thread_pool, origin, &[tile()], MapMode::Continuous);
    if K::INFO.layer_kind == SourceKind::Geometry {
        set_water_layer(&mut tiles);
    }
    tiles
}

/// Has `tiles` lay out a circle layer of the "water" source layer.
fn set_water_layer(tiles: &mut OriginTiles) {
    let registry = registry();
    let mut layer = Layer::new(registry.style(), "circle", "water", "source").unwrap();
    layer.set_source_layer("water");
    tiles.set_layers(&registry, &[layer]).unwrap();
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

/// A tile required again keeps network data without an expiry, as a local
/// file's: it doesn't request it again. Data that expires is requested
/// again, with its validators and its expiry, which the online file source
/// holds the request until, and a "not modified" answer keeps the data.
#[test]
fn a_tile_required_again_asks_again_for_data_that_expires() {
    let test = TileTest::new();
    let later = Timestamp::from_secs(i64::MAX);
    let mut until_later = water();
    until_later.caching.expires = Some(later);
    for (response, expires) in
        [(water(), None), (until_later, Some(later)), (expired(water()), Some(Timestamp::from_secs(0)))]
    {
        let (mut tiles, file_source) = held_tile::<VectorKind>(&test, tiles_tileset());
        tiles.set_necessity(&tile(), TileNecessity::Required);
        answer(&mut tiles, &file_source, response);
        assert!(tiles.is_complete(&tile()));

        tiles.set_necessity(&tile(), TileNecessity::Optional);
        tiles.set_necessity(&tile(), TileNecessity::Required);
        assert_eq!(file_source.pending(), usize::from(expires.is_some()), "a request only for data that expires");
        if expires.is_some() {
            let resource = file_source.resource();
            assert_eq!(resource.prior_caching.expires, expires, "held until the data expires");
            assert_eq!(resource.prior_etag.as_deref(), Some("a"));
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
    let responder = file_source.take();
    responder.send(not_modified());
    tiles.update();
    tiles.update();
    assert_eq!(actions(&mut tiles), [RequestedFromNetwork, LoadFromNetwork], "no new contents");
    assert!(tiles.is_complete(&tile()));
    assert!(!responder.is_cancelled(), "the request stays open for the next revalidation");
    assert_eq!(file_source.pending(), 0);

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
    held_source_at(test, "{z}/{x}/{y}")
}

/// A vector source "source" whose tiles are at `template`, as
/// [`held_source`].
fn held_source_at(test: &TileTest, template: &str) -> (Source, OneSource, Arc<Held>) {
    let tileset = PartialTileset { tiles: Some(vec![template.to_owned()]), ..Default::default() };
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
    View { state, mode: MapMode::Continuous, pixel_ratio: 1.0, file_source: Some(file_source), needs_relayout: false }
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

/// A tile in a new world copy whose resource is another than its copy's,
/// as after the pixel ratio changed, doesn't take the copy's data and
/// validators: it loads its own.
#[test]
fn a_world_copy_of_another_resource_loads_its_own() {
    let test = TileTest::new();
    let (source, mut tiles, file_source) = held_source_at(&test, "{z}/{x}/{y}{ratio}");
    let state = view_state(0.0);
    tiles.update(&source, &held_view(&state, &file_source));
    assert_eq!(file_source.requests("0/0/0").len(), 1);
    file_source.respond(water());
    while !tiles.is_loaded() {
        tiles.step();
    }

    // 170 degrees east shows tile 0/0/0 of the next world copy too.
    let mut east = TransformState::clone(&state);
    east.set_lat_lng_zoom(&LatLng::new(0.0, 170.0).unwrap(), 0.0);
    tiles.update(&source, &View { pixel_ratio: 2.0, ..held_view(&east, &file_source) });
    let requests = file_source.requests("0/0/0@2x");
    assert_eq!(requests.len(), 1, "the new world copy loads its own tile");
    assert_eq!(requests[0].0.prior_etag, None, "with no validators of the copy's");
    assert!(!tiles.is_loaded(), "the new world copy waits for its own tile");
}

/// The tile at a tileset's last zoom and the tiles cut from it share what
/// either brought fresh from the network: whichever comes second takes it
/// from memory, without a request.
#[test]
fn a_tile_at_the_last_zoom_and_its_cut_tiles_share_their_data() {
    let test = TileTest::new();
    let (whole, cut) = (OverscaledTileID::from_zxy(0, 0, 0), OverscaledTileID::from_zxy(1, 0, 0));
    for (first, second) in [(whole, cut), (cut, whole)] {
        let file_source = Arc::new(Held::default());
        let file_source_dyn: Arc<dyn FileSource> = Arc::<Held>::clone(&file_source);
        let tileset = Tileset { zoom_range: Range::new(0, 0), ..tiles_tileset() };
        let origin = TilesetOrigin::<VectorKind>::loaded(tileset, VectorTileEncoding::Mvt, Some(file_source_dyn));
        let mut tiles = OriginTiles::new(&test.thread_pool, origin, &[whole, cut], MapMode::Static);
        set_water_layer(&mut tiles);
        tiles.set_necessity(&first, TileNecessity::Required);
        answer(&mut tiles, &file_source, water());
        assert!(tiles.is_complete(&first));

        tiles.set_necessity(&second, TileNecessity::Required);
        assert_eq!(file_source.pending(), 0, "{second:?} after {first:?}");
        tiles.update();
        tiles.update();
        assert!(tiles.is_complete(&second));
    }
}

/// A connection error.
fn connection_error() -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::Connection, "offline")));
    response
}

/// A required tile's network request stays open, and the tile takes each
/// answer the file source gives it: after errors, of which a streak fails
/// the tile once, its retry's data; when the data expires, a revalidation's
/// "not modified", which keeps the data, the same bytes again, which keep it
/// too, an error, which keeps it, or new data, which lays out again.
#[test]
fn a_tile_takes_every_answer_to_its_open_request() {
    use TileOperation::{EndParse, Error, LoadFromNetwork, RequestedFromNetwork, StartParse};

    let test = TileTest::new();
    let (mut tiles, file_source) = held_tile::<VectorKind>(&test, tiles_tileset());
    tiles.set_necessity(&tile(), TileNecessity::Required);
    let responder = file_source.take();
    responder.send(connection_error());
    responder.send(connection_error());
    tiles.update();
    assert_eq!(actions(&mut tiles), [StartParse, RequestedFromNetwork, Error], "one error for the streak");
    assert!(tiles.is_complete(&tile()), "a failed tile has done what it can for now");

    // The retry's answer.
    responder.send(water());
    tiles.update();
    tiles.update();
    assert_eq!(actions(&mut tiles), [LoadFromNetwork, StartParse, EndParse]);
    assert!(tiles.is_renderable(&tile()) && tiles.is_complete(&tile()));

    // Revalidations: unchanged, the same bytes again, failed, then new data.
    responder.send(not_modified());
    tiles.update();
    assert_eq!(actions(&mut tiles), [LoadFromNetwork]);
    responder.send(water());
    tiles.update();
    assert_eq!(actions(&mut tiles), [LoadFromNetwork], "the same bytes aren't laid out again");
    responder.send(connection_error());
    tiles.update();
    assert_eq!(actions(&mut tiles), [Error]);
    assert!(tiles.is_renderable(&tile()), "a failed revalidation keeps the data");
    responder.send(water_changed());
    tiles.update();
    tiles.update();
    assert_eq!(actions(&mut tiles), [LoadFromNetwork, StartParse, EndParse]);
    assert!(tiles.is_renderable(&tile()) && tiles.is_complete(&tile()));

    assert!(!responder.is_cancelled(), "the request stays open");
    assert_eq!(file_source.pending(), 0, "no new request");
    tiles.set_necessity(&tile(), TileNecessity::Optional);
    assert!(responder.is_cancelled(), "an optional tile drops it");
}

/// A tile that becomes optional while its failed request waits for the file
/// source's retry cancels it, and requests anew when required again.
#[test]
fn an_optional_tile_stops_retrying() {
    let test = TileTest::new();
    let (mut tiles, file_source) = held_tile::<VectorKind>(&test, tiles_tileset());
    tiles.set_necessity(&tile(), TileNecessity::Required);
    let responder = file_source.take();
    responder.send(connection_error());
    tiles.update();

    tiles.set_necessity(&tile(), TileNecessity::Optional);
    assert!(responder.is_cancelled());
    tiles.set_necessity(&tile(), TileNecessity::Required);
    assert_eq!(file_source.pending(), 1, "a new request");
}

/// A required vector tile whose file source's cache answers with `cached`.
fn tile_with_cached(test: &TileTest, file_source: &Arc<Held>, cached: Response) -> OriginTiles {
    let mut tiles = held_tile_from::<VectorKind>(test, tiles_tileset(), VectorTileEncoding::Mvt, file_source);
    tiles.set_necessity(&tile(), TileNecessity::Required);
    assert_eq!(LoadingMethod::CacheOnly, file_source.respond(cached).loading_method);
    tiles.update();
    tiles.update();
    tiles
}

/// `response`, expired, which must revalidate before it shows.
fn must_revalidate(response: Response) -> Response {
    let mut response = expired(response);
    response.caching.must_revalidate = true;
    response
}

/// A stale copy from the cache that must revalidate waits for the network,
/// which a 304 shows it after.
#[test]
fn a_stale_copy_that_must_revalidate_waits_for_the_network() {
    let test = TileTest::new();
    let file_source = Arc::new(Held::with_cache());
    let mut tiles = tile_with_cached(&test, &file_source, must_revalidate(water()));
    assert!(!tiles.is_renderable(&tile()), "held back");
    let network = file_source.resource();
    assert_eq!((LoadingMethod::NetworkOnly, Priority::Regular), (network.loading_method, network.priority));
    assert_eq!(Some("a"), network.prior_etag.as_deref());

    answer(&mut tiles, &file_source, not_modified());
    assert!(tiles.is_renderable(&tile()) && tiles.is_complete(&tile()));
}

/// A stale copy shows while it revalidates, and stays when the server can't
/// be reached, which the tile reports; one that must revalidate never shows.
#[test]
fn a_stale_copy_shows_until_it_revalidates() {
    use TileOperation::Error;

    let test = TileTest::new();
    let file_source = Arc::new(Held::with_cache());
    let mut tiles = tile_with_cached(&test, &file_source, expired(water()));
    assert!(tiles.is_renderable(&tile()), "shown at once");
    let network = file_source.resource();
    assert_eq!((LoadingMethod::NetworkOnly, Priority::Low), (network.loading_method, network.priority));
    tiles.take_events();
    file_source.respond(connection_error());
    tiles.update();
    assert!(tiles.is_renderable(&tile()));
    assert_eq!(actions(&mut tiles), [Error]);

    let mut tiles = tile_with_cached(&test, &file_source, must_revalidate(water()));
    tiles.take_events();
    file_source.respond(connection_error());
    tiles.update();
    assert!(!tiles.is_renderable(&tile()));
    assert_eq!(actions(&mut tiles), [Error]);
}

/// The format that a request for a tile of a tileset of the kind `K`, in
/// `encoding`, names.
fn requested_format<K: TilesetKind>(encoding: VectorTileEncoding) -> Option<TileFormat> {
    let test = TileTest::new();
    let file_source = Arc::new(Held::default());
    let mut tiles = held_tile_from::<K>(&test, tiles_tileset(), encoding, &file_source);
    tiles.set_necessity(&tile(), TileNecessity::Required);
    file_source.resource().tile_data.map(|tile| tile.format)
}

#[test]
fn a_tile_request_names_the_format_its_source_decodes() {
    let mlt = VectorTileEncoding::Mlt { fast_pfor_enabled: false };
    assert_eq!(Some(TileFormat::Mvt), requested_format::<VectorKind>(VectorTileEncoding::Mvt));
    assert_eq!(Some(TileFormat::Mlt), requested_format::<VectorKind>(mlt));
    assert_eq!(Some(TileFormat::Image), requested_format::<RasterKind>(VectorTileEncoding::Mvt));
    assert_eq!(Some(TileFormat::Image), requested_format::<RasterDemKind>(VectorTileEncoding::Mvt));
}
