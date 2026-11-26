use mln_style::error::Error;
use mln_style::layer::SourceKind;

use crate::exec::pool::TaskGroup;
use crate::map::MapMode;
use crate::query::SourceQueryOptions;
use crate::tests::OneTile;
use crate::tile::OverscaledTileID;
use crate::worker::tile_workers::InlineWorkers;

struct VectorTileTest {
    thread_pool: TaskGroup,
}

impl VectorTileTest {
    fn new() -> Self {
        VectorTileTest { thread_pool: TaskGroup::new(&crate::tests::pool()) }
    }

    fn mvt_tile(&self) -> OneTile {
        let store = crate::tests::tile_store(&self.thread_pool, Box::new(InlineWorkers));
        OneTile::new(store, SourceKind::Geometry, OverscaledTileID::from_zxy(0, 0, 0), MapMode::Continuous)
    }
}

impl Drop for VectorTileTest {
    fn drop(&mut self) {
        self.thread_pool.wait();
    }
}

// gtest: VectorTile.setError
#[test]
fn set_error() {
    let test = VectorTileTest::new();
    let mut tiles = test.mvt_tile();
    tiles.with(|tile| tile.fail(Error::native("test")));
    assert!(!tiles.tile().is_renderable());
    assert!(tiles.tile().is_settled());
    assert!(tiles.tile().is_complete());
}

// gtest: VectorTile.onError
#[test]
fn on_error() {
    let test = VectorTileTest::new();
    let mut tiles = test.mvt_tile();
    tiles.with(|tile| tile.fail(Error::native("test")));
    assert!(!tiles.tile().is_renderable());
    assert!(tiles.tile().is_settled());
    assert!(tiles.tile().is_complete());
}

// gtest: VectorTile.Issue8542
#[test]
fn issue_8542() {
    let test = VectorTileTest::new();
    let tiles = test.mvt_tile();

    // Query before data is set
    let source = tiles.store.source(tiles.key).expect("the source");
    let features = source.query_source_features(&SourceQueryOptions::new(Some(vec!["layer".to_owned()]), None), None);
    assert_eq!(features.unwrap(), Vec::new());
}
