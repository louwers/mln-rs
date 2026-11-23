// The rest of test/tile/custom_geometry_tile.test.cpp, about the provider
// that answers a custom source's tiles, is in mln_sources. Here the test
// gives the tile its data.

use std::sync::Arc;

use feature::Feature;
use geo_types::Point;
use mln_style::layer::Layer;
use mln_style::layer_type::LayerStyle;
use mln_tile_id::OverscaledTileID;

use super::RasterTileTest;
use crate::Registry;
use crate::erased_layer::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::query::SourceQueryOptions;
use crate::tests::OneTile;
use crate::tests::fixture_layer::Dot;
use crate::tests::scripted_origin::Script;
use crate::tile::contents::Contents;
use crate::tile::tile::TileKind;
use crate::tile::tile_workers::InlineWorkers;

/// The root tile's id.
fn root() -> OverscaledTileID {
    OverscaledTileID::from_zxy(0, 0, 0)
}

// gtest: CustomGeometryTile.InvokeTileChanged
#[test]
fn invoke_tile_changed() {
    let test = RasterTileTest::new();
    let store = crate::tests::tile_store(&test.thread_pool, Box::new(InlineWorkers));
    let mut tiles = OneTile::with_script(
        store,
        TileKind::Geometry,
        true,
        &Script::default(),
        &[root()],
        mln_map_types::MapMode::Static,
    );

    let layer = Layer::new(Registry::with_builtins().style(), "dot", "circle", "source").unwrap();
    let layer_properties: Arc<dyn ErasedEvaluatedLayer> =
        Arc::new(EvaluatedLayer::<Dot>::new(LayerStyle::recover(layer.style()).unwrap()));

    tiles.set_style(&crate::tests::style(std::slice::from_ref(&layer_properties)));
    assert!(tiles.tile().contents().is_none());

    tiles.set_data(Some(mln_tile::TileData::Features(Arc::new(vec![Feature {
        geometry: Some(Point::new(0i16, 0).into()),
        ..Feature::default()
    }]))));
    while !tiles.tile().is_complete() {
        test.local_pool.run_all();
        tiles.update();
    }
    // Once present, the bucket should never "disappear", which would cause
    // flickering.
    assert!(tiles.with(|tile| tile.parts().1.layer_properties_updated(&layer_properties)));

    let Some(Contents::Geometry(contents)) = tiles.tile().contents() else {
        panic!("a layout");
    };
    let mut result = Vec::new();
    contents.query_source_features(&root(), true, &mut result, &SourceQueryOptions::default(), None).unwrap();
    assert_eq!(result.len(), 1);
}
