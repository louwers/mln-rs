//! Port-only: what a tileset source's tiles load, beyond what the engine's
//! tests cover with their fixture source.

use std::sync::Arc;

use mln_map_types::MapMode;
use mln_renderer::source::{TileKind, TileNecessity};
use mln_renderer::testing::OriginTiles;
use mln_storage::{FileSource, Resource, Responder, Response};
use mln_style::Layer;
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::Timestamp;
use mln_util::range::Range;
use mln_util::tileset::Tileset;
use parking_lot::Mutex;

use super::{TileTest, registry};
use crate::tileset::VectorKind;
use crate::tileset::origin::{TilesetOrigin, VectorTileEncoding};

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
