// VectorTile.setError, VectorTile.onError and VectorTile.Issue8542 need
// VectorMVTTile, TileParameters and the renderer: they are in
// crates/mln_renderer/tests/vector_tile_test.rs.

use std::path::Path;
use std::sync::Arc;

use feature::Identifier;
use mln_tile_id::GeometryTileData;
use mln_util::geometry::FeatureType;

use crate::{VectorMLTTileData, VectorMVTTileData};

fn read_file(path: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(path)).unwrap()
}

// gtest: VectorTileData.ParseResults
#[test]
fn parse_results() {
    let data = VectorMVTTileData::new(Arc::new(read_file("test/fixtures/map/issue12432/0-0-0.mvt")));

    let layer_names = data.layer_names().unwrap();
    assert_eq!(layer_names.len(), 2);
    assert_eq!(layer_names[0], "admin");
    assert_eq!(layer_names[1], "water");

    assert!(data.get_layer("invalid").unwrap().is_none());

    let layer = data.get_layer("admin").unwrap().unwrap();
    assert_eq!(layer.get_name(), "admin");
    assert_eq!(layer.feature_count(), 17154);

    // ASSERT_THROW(layer->getFeature(17154u), std::out_of_range)
    let typed = data.try_get_layer("admin").unwrap().unwrap();
    assert!(matches!(typed.try_get_feature(17154), Err(crate::Error::OutOfRange(_))));
    assert_eq!(
        layer.get_feature(17154).err().unwrap().to_string(),
        "vector::_M_range_check: __n (which is 17154) >= this->size() (which is 17154)"
    );

    let feature = layer.get_feature(0).unwrap();
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
        let data = VectorMLTTileData::new(Arc::new(read_file(&path)), test_case.fast_pfor_enabled);

        let layer_names = data.layer_names();

        if !test_case.fast_pfor_enabled && test_case.use_fast_pfor {
            // If fast PFOR is not enabled, the fast PFOR tile should fail to parse
            assert_eq!(layer_names.len(), 0);
            continue;
        }

        // MLT layers are presented in the order they're encountered, not sorted alphabetically like MVT layers
        assert_eq!(layer_names.len(), 2);
        assert_eq!(layer_names[0], "water");
        assert_eq!(layer_names[1], "admin");

        assert!(data.get_layer("invalid").unwrap().is_none());

        let layer = data.get_layer("admin").unwrap().unwrap();
        assert_eq!(layer.get_name(), "admin");
        assert_eq!(layer.feature_count(), 17154);

        let feature = layer.get_feature(0).unwrap();
        assert_eq!(feature.get_type(), FeatureType::LineString);
        assert!(matches!(feature.get_id(), Identifier::Uint(_)));
        assert_eq!(feature.get_id(), Identifier::Uint(1));

        let properties = feature.get_properties();
        assert_eq!(properties.len(), 3);
        assert_eq!(properties.get("disputed"), feature.get_value("disputed").as_ref());

        assert_eq!(feature.get_value("invalid"), None);
    }
}
