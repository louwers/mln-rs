//! Port-only: what a tileset source's tiles load, beyond what the engine's
//! tests cover with their fixture source.

use std::sync::Arc;

use mln_map_types::MapMode;
use mln_renderer::source::{TileKind, TileNecessity};
use mln_renderer::testing::OriginTiles;
use mln_storage::response::Reason;
use mln_storage::{FileSource, Resource, ResourceError, Responder, Response};
use mln_style::Layer;
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::Timestamp;
use mln_util::range::Range;
use mln_util::tileset::Tileset;
use parking_lot::Mutex;

use super::{TileTest, errors, registry};
use crate::tileset::origin::{TilesetOrigin, VectorTileEncoding};
use crate::tileset::{RasterKind, TilesetKind, VectorKind};

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
        let mut tiles =
            OriginTiles::new(&test.thread_pool, TileKind::Geometry, false, origin, &[left, right], MapMode::Static);
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
fn held_tile<K: TilesetKind>(test: &TileTest, kind: TileKind, tileset: Tileset) -> (OriginTiles, Arc<Held>) {
    let file_source = Arc::new(Held::default());
    let file_source_dyn: Arc<dyn FileSource> = Arc::<Held>::clone(&file_source);
    let origin = TilesetOrigin::<K>::loaded(tileset, VectorTileEncoding::Mvt, Some(file_source_dyn));
    let mut tiles = OriginTiles::new(&test.thread_pool, kind, false, origin, &[tile()], MapMode::Continuous);
    if kind == TileKind::Geometry {
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
        (TileKind::Geometry, no_content(), tiles_tileset(), true),
        (TileKind::Geometry, not_found(), tiles_tileset(), true),
        (TileKind::Geometry, no_content(), sparse.clone(), false),
        (TileKind::Raster, no_content(), tiles_tileset(), true),
        (TileKind::Raster, not_found(), tiles_tileset(), false),
        (TileKind::Raster, no_content(), sparse, false),
    ];
    for (i, (kind, response, tileset, renderable)) in cases.into_iter().enumerate() {
        let (mut tiles, file_source) = match kind {
            TileKind::Geometry => held_tile::<VectorKind>(&test, kind, tileset),
            _ => held_tile::<RasterKind>(&test, kind, tileset),
        };
        tiles.set_necessity(&tile(), TileNecessity::Required);
        answer(&mut tiles, &file_source, response);
        assert!(tiles.is_complete(&tile()), "case {i}");
        assert_eq!(tiles.is_renderable(&tile()), renderable, "case {i}");
        assert_eq!(errors(&mut tiles), Vec::<String>::new(), "case {i}");
    }
}

/// A failed load reports its error and asks again only as the engine
/// retries it.
#[test]
fn a_failed_load_is_requested_again_on_retry() {
    let test = TileTest::new();
    let (mut tiles, file_source) = held_tile::<VectorKind>(&test, TileKind::Geometry, tiles_tileset());
    tiles.set_necessity(&tile(), TileNecessity::Required);
    let mut failed = Response::new();
    failed.error = Some(Box::new(ResourceError::new(Reason::Other, "failed")));
    answer(&mut tiles, &file_source, failed);
    assert_eq!(errors(&mut tiles), ["failed"]);
    assert_eq!(file_source.pending(), 0);

    tiles.retry_failed();
    assert_eq!(file_source.pending(), 1);
    answer(&mut tiles, &file_source, water());
    assert!(tiles.is_complete(&tile()));
    assert_ne!(tiles.features(&tile(), "water").unwrap(), Vec::new());
}

/// A tile required again loads again only if its data isn't fresh from the
/// network. The reload asks with the data's validators, and a "not
/// modified" answer keeps the data.
#[test]
fn a_tile_required_again_reloads_only_stale_data() {
    let test = TileTest::new();
    for (response, reloads) in [(water(), false), (expired(water()), true)] {
        let (mut tiles, file_source) = held_tile::<VectorKind>(&test, TileKind::Geometry, tiles_tileset());
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

/// A tile's requests start in the tile's place among the requests that
/// wait, after those of anything else, which leave theirs at zero.
#[test]
fn a_request_takes_the_tiles_order() {
    let test = TileTest::new();
    let (mut tiles, file_source) = held_tile::<VectorKind>(&test, TileKind::Geometry, tiles_tileset());
    tiles.set_necessity(&tile(), TileNecessity::Required);
    assert!(file_source.resource().order.get() > 0);
}
