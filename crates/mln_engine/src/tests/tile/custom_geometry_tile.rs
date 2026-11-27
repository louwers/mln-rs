// The rest of test/tile/custom_geometry_tile.test.cpp, about the provider
// that answers a custom source's tiles, is in mln_c's custom sources. Here
// the test gives the tile the features a custom geometry source cuts.

use std::sync::Arc;

use geo_types::Point;
use mln_style::feature::Feature;
use mln_style::layer::{Layer, LayerStyle, SourceKind};
use mln_style::layers::Circle;

use super::{RasterTileTest, local_tiles};
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::query::SourceQueryOptions;
use crate::tile::OverscaledTileID;
use crate::tile_store::contents::Contents;

/// The root tile's id.
fn root() -> OverscaledTileID {
    OverscaledTileID::from_zxy(0, 0, 0)
}

// gtest: CustomGeometryTile.InvokeTileChanged
#[test]
fn invoke_tile_changed() {
    let test = RasterTileTest::new();
    let mut tiles = local_tiles(&test, SourceKind::Geometry, root());

    let layer = Layer::new(crate::tests::registry().style(), "circle", "circle", "source").unwrap();
    let layer_properties: Arc<dyn ErasedEvaluatedLayer> =
        Arc::new(EvaluatedLayer::<Circle>::new(LayerStyle::recover(layer.style()).unwrap()));

    tiles.set_style(&crate::tests::style(std::slice::from_ref(&layer_properties)));
    assert!(tiles.tile().contents().is_none());

    tiles.set_data(Some(crate::tile::TileData::Features(Arc::new(vec![Feature {
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
    contents.query_source_features(&root(), &mut result, &SourceQueryOptions::default(), None).unwrap();
    assert_eq!(result.len(), 1);
}
