// VectorTile.setError, VectorTile.onError and VectorTile.Issue8542 need
// VectorMVTTile, TileParameters and the renderer: they are in
// crates/mln_engine/tests/vector_tile_test.rs.

use std::path::Path;
use std::sync::Arc;

use mln_style::feature::{GeometryTileFeature, Identifier};
use mln_style::geometry::FeatureType;

use crate::tile::{MltTile, MvtTile, TileData, TileError};

fn read_file(path: &str) -> Vec<u8> {
    std::fs::read(Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(path)).unwrap()
}

// gtest: VectorTileData.ParseResults
#[test]
fn parse_results() {
    let tile = MvtTile::new(Arc::new(read_file("test/fixtures/map/issue12432/0-0-0.mvt")));
    let layer_names = tile.layer_names().unwrap();
    assert_eq!(layer_names.len(), 2);
    assert_eq!(layer_names[0], "admin");
    assert_eq!(layer_names[1], "water");

    let data = TileData::Mvt(tile);
    assert!(data.layer("invalid").unwrap().is_none());

    let layer = data.layer("admin").unwrap().unwrap();
    assert_eq!(layer.name(), "admin");
    assert_eq!(layer.len(), 17154);

    let error = layer.feature(17154).err().unwrap();
    assert!(matches!(error, TileError::OutOfRange(_)));
    assert_eq!(error.to_string(), "vector::_M_range_check: __n (which is 17154) >= this->size() (which is 17154)");

    let feature = layer.feature(0).unwrap();
    assert_eq!(feature.get_type(), FeatureType::LineString);
    assert!(matches!(feature.get_id(), Identifier::Uint(_)));
    assert_eq!(feature.get_id(), Identifier::Uint(1));

    let properties = feature.get_properties();
    assert_eq!(properties.len(), 3);
    assert_eq!(properties.get("disputed"), feature.get_value("disputed").as_ref());

    assert_eq!(feature.get_value("invalid"), None);
}

// gtest: VectorTileData.MLTParseResults
#[test]
fn mlt_parse_results() {
    struct SubCase {
        use_fast_pfor: bool,
        fast_pfor_enabled: bool,
    }
    for test_case in [
        SubCase { use_fast_pfor: false, fast_pfor_enabled: false },
        SubCase { use_fast_pfor: false, fast_pfor_enabled: true },
        SubCase { use_fast_pfor: true, fast_pfor_enabled: true },
        SubCase { use_fast_pfor: true, fast_pfor_enabled: false },
    ] {
        let path = format!(
            "test/fixtures/map/issue12432/0-0-0{}",
            if test_case.use_fast_pfor { "-fastpfor.mlt" } else { ".mlt" }
        );
        let tile = MltTile::new(Arc::new(read_file(&path)), test_case.fast_pfor_enabled);
        let layer_names = tile.layer_names();

        if !test_case.fast_pfor_enabled && test_case.use_fast_pfor {
            // If fast PFOR is not enabled, the fast PFOR tile should fail to parse
            assert_eq!(layer_names.len(), 0);
            continue;
        }

        // MLT layers are presented in the order they're encountered, not sorted alphabetically like MVT layers
        assert_eq!(layer_names.len(), 2);
        assert_eq!(layer_names[0], "water");
        assert_eq!(layer_names[1], "admin");

        let data = TileData::Mlt(tile);
        assert!(data.layer("invalid").unwrap().is_none());

        let layer = data.layer("admin").unwrap().unwrap();
        assert_eq!(layer.name(), "admin");
        assert_eq!(layer.len(), 17154);
        assert!(matches!(layer.feature(17154), Err(TileError::OutOfRange(_))));

        let feature = layer.feature(0).unwrap();
        assert_eq!(feature.get_type(), FeatureType::LineString);
        assert!(matches!(feature.get_id(), Identifier::Uint(_)));
        assert_eq!(feature.get_id(), Identifier::Uint(1));

        let properties = feature.get_properties();
        assert_eq!(properties.len(), 3);
        assert_eq!(properties.get("disputed"), feature.get_value("disputed").as_ref());

        assert_eq!(feature.get_value("invalid"), None);
    }
}
