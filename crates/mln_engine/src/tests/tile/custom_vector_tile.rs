// The rest of test/tile/custom_vector_tile.test.cpp, about the provider
// that answers a custom source's tiles, is in mln_c's custom sources. Here
// the test gives the tile the bytes a custom vector source decodes.

use std::sync::Arc;

use mln_style::layer::{Layer, LayerStyle, SourceKind};
use mln_style::layers::Fill;

use super::{RasterTileTest, local_tiles};
use crate::Registry;
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::query::SourceQueryOptions;
use crate::tile::{MltTile, OverscaledTileID, TileData};
use crate::tile_store::contents::Contents;

// Port-only: a tile decodes MLT data as MLT (ffi patch 0052).
#[test]
fn set_mlt_tile_data() {
    let test = RasterTileTest::new();
    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tiles = local_tiles(&test, SourceKind::Geometry, root);

    let mut layer = Layer::new(Registry::with_builtins().style(), "fill", "water", "source").unwrap();
    layer.set_source_layer("water");
    let layer_properties: Arc<dyn ErasedEvaluatedLayer> =
        Arc::new(EvaluatedLayer::<Fill>::new(LayerStyle::recover(layer.style()).unwrap()));
    tiles.set_style(&crate::tests::style(&[layer_properties]));

    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("testdata/test/fixtures/map/issue12432/0-0-0.mlt");
    let data = Arc::new(std::fs::read(path).unwrap());
    tiles.set_data(Some(TileData::Mlt(MltTile::new(data, false))));
    while !tiles.tile().is_complete() {
        test.local_pool.run_all();
        tiles.update();
    }

    let options = SourceQueryOptions { source_layers: Some(vec!["water".to_owned()]), filter: None };
    let Some(Contents::Geometry(contents)) = tiles.tile().contents() else {
        panic!("a layout");
    };
    let mut result = Vec::new();
    contents.query_source_features(&root, &mut result, &options, None).unwrap();
    assert_ne!(result, Vec::<crate::tile::feature::Feature>::new());
}
