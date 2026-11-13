// Ports vendor/supercluster/test/test.cpp over test/fixtures/places.json,
// read in place (parseFeatures reads the same properties the same way).

use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use geometry::{Geometry, Point};
use serde_json::Value as Json;

use crate::{Options, Supercluster};

fn parse_features() -> Vec<Feature<f64>> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/vendor/supercluster/test/fixtures/places.json");
    let d: Json = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();

    let json_features = d["features"].as_array().unwrap();
    let mut features = Vec::with_capacity(json_features.len());

    for itr in json_features {
        let json_coords = &itr["geometry"]["coordinates"];
        let lng = json_coords[0].as_f64().unwrap();
        let lat = json_coords[1].as_f64().unwrap();
        let mut feature = Feature::new(Geometry::Point(Point::new(lng, lat)));
        let properties = &itr["properties"];
        for key in
            ["name", "scalerank", "lat_y", "long_x", "region", "featureclass", "comment", "name_alt", "subregion"]
        {
            if let Some(value) = properties.get(key) {
                let v = match value {
                    Json::Null => Some(Value::String("null".to_owned())),
                    Json::String(s) => Some(Value::String(s.clone())),
                    Json::Number(n) if n.is_u64() => Some(Value::Uint(n.as_u64().unwrap())),
                    Json::Number(n) if n.is_f64() => Some(Value::Double(n.as_f64().unwrap())),
                    _ => None,
                };
                if let Some(v) = v {
                    feature.properties.insert(key.to_owned(), v);
                }
            }
        }
        features.push(feature);
    }

    features
}

fn uint(props: &PropertyMap, key: &str) -> u64 {
    match props.get(key) {
        Some(Value::Uint(u)) => *u,
        v => panic!("{key}: {v:?}"),
    }
}

fn string<'a>(props: &'a PropertyMap, key: &str) -> &'a str {
    match props.get(key) {
        Some(Value::String(s)) => s,
        v => panic!("{key}: {v:?}"),
    }
}

fn is_cluster(props: &PropertyMap) -> bool {
    matches!(props.get("cluster"), Some(Value::Bool(true)))
}

fn sum_options(radius: Option<u16>) -> Options {
    let map = Arc::new(|properties: &PropertyMap| -> PropertyMap {
        let mut ret = PropertyMap::new();
        if let Some(v) = properties.get("scalerank") {
            let Value::Uint(u) = v else { panic!("bad_variant_access") };
            ret.insert("sum".to_owned(), Value::Uint(*u));
        }
        ret
    });
    let reduce = Arc::new(|to_update: &mut PropertyMap, to_fill: &PropertyMap| {
        if let (Some(Value::Uint(a)), Some(Value::Uint(b))) = (to_update.get("sum"), to_fill.get("sum")) {
            let sum = a + b;
            to_update.insert("sum".to_owned(), Value::Uint(sum));
        }
    });
    let mut options = Options { map, reduce: Some(reduce), ..Options::default() };
    if let Some(r) = radius {
        options.radius = r;
    }
    options
}

#[test]
fn test_1_default_options() {
    let features = parse_features();
    let index = Supercluster::new(&features, Options::default()).unwrap();

    let tile = index.get_tile(0, 0, 0);
    assert_eq!(tile.len(), 39);

    let mut num_points = 0;
    for f in &tile {
        if is_cluster(&f.properties) {
            num_points += uint(&f.properties, "point_count");
        } else {
            num_points += 1;
        }
    }

    assert_eq!(num_points, 196);

    let children = index.get_children(1).unwrap();

    assert_eq!(children.len(), 4);
    assert_eq!(uint(&children[0].properties, "point_count"), 6);
    assert_eq!(uint(&children[1].properties, "point_count"), 7);
    assert_eq!(uint(&children[2].properties, "point_count"), 2);
    assert_eq!(string(&children[3].properties, "name"), "Bermuda Islands");

    assert_eq!(index.get_cluster_expansion_zoom(1).unwrap(), 1);
    assert_eq!(index.get_cluster_expansion_zoom(33).unwrap(), 1);
    assert_eq!(index.get_cluster_expansion_zoom(353).unwrap(), 2);
    assert_eq!(index.get_cluster_expansion_zoom(833).unwrap(), 2);
    assert_eq!(index.get_cluster_expansion_zoom(1857).unwrap(), 3);

    let leaves = index.get_leaves(1, 10, 5).unwrap();

    let names: Vec<&str> = leaves.iter().map(|l| string(&l.properties, "name")).collect();
    assert_eq!(
        names,
        [
            "Niagara Falls",
            "Cape San Blas",
            "Cape Sable",
            "Cape Canaveral",
            "San  Salvador",
            "Cabo Gracias a Dios",
            "I. de Cozumel",
            "Grand Cayman",
            "Miquelon",
            "Cape Bauld",
        ]
    );
}

#[test]
fn test_2_expansion_zoom() {
    let features = parse_features();
    let options2 = Options { radius: 60, extent: 256, max_zoom: 4, ..Options::default() };
    let index2 = Supercluster::new(&features, options2).unwrap();
    assert_eq!(index2.get_cluster_expansion_zoom(2436).unwrap(), 5);
}

#[test]
fn test_3_map_reduce() {
    let features = parse_features();
    let index3 = Supercluster::new(&features, sum_options(None)).unwrap();

    let tile3 = index3.get_tile(0, 0, 0);
    assert_ne!(tile3, Vec::<Feature<i16>>::new());
    assert!(tile3[0].properties.contains_key("sum"));
    assert_eq!(uint(&tile3[0].properties, "sum"), 69);
}

#[test]
fn test_4_map_reduce_radius() {
    let features = parse_features();
    let index4 = Supercluster::new(&features, sum_options(Some(100))).unwrap();

    let expect_vec0: Vec<u64> = vec![298, 122, 12, 36, 98, 7, 24, 8, 125, 98, 125, 12, 36, 8];
    let expect_vec1: Vec<u64> = vec![146, 84, 63, 23, 34, 12, 19, 29, 8, 8, 80, 35];
    let sums = |tile: &[Feature<i16>]| -> Vec<u64> {
        tile.iter().filter(|f| f.properties.contains_key("sum")).map(|f| uint(&f.properties, "sum")).collect()
    };
    assert_eq!(sums(&index4.get_tile(0, 0, 0)), expect_vec0);
    assert_eq!(sums(&index4.get_tile(1, 0, 0)), expect_vec1);
}

#[test]
fn test_5_min_points() {
    let features = parse_features();
    let options5 = Options { min_points: 5, ..Options::default() };
    let index5 = Supercluster::new(&features, options5).unwrap();

    let tile5 = index5.get_tile(0, 0, 0);
    assert_eq!(tile5.len(), 49);

    let mut num_points1 = 0;
    for f in &tile5 {
        if is_cluster(&f.properties) {
            let point_count = uint(&f.properties, "point_count");
            assert!(point_count >= 5);
            num_points1 += point_count;
        } else {
            num_points1 += 1;
        }
    }

    assert_eq!(num_points1, 195);
}

#[test]
fn test_generate_id() {
    let features = parse_features();
    let options = Options { generate_id: true, ..Options::default() };
    let index = Supercluster::new(&features, options).unwrap();
    let ids: Vec<u64> = index
        .get_tile(0, 0, 0)
        .iter()
        .filter(|f| !f.properties.contains_key("cluster"))
        .map(|f| match f.id {
            Identifier::Uint(u) => u,
            ref id => panic!("{id:?}"),
        })
        .collect();

    assert_eq!(ids, vec![12, 20, 21, 22, 24, 28, 30, 62, 81, 118, 119, 125, 81, 118]);
}
