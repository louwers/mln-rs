//! Clusters testdata/supercluster/places.json, the places of supercluster's tests. The
//! expectations are GL JS's supercluster's (8.0.1), whose cluster IDs are
//! these plus the number of points.

use geo_types::Point;
use mln_style::feature::{Feature, Identifier, PropertyMap, Value};
use serde_json::Value as Json;

use super::{Aggregate, Clusters, Error, MAX_ZOOM, Options, abbreviate};
use crate::sources::geojson::TileFeatures;

const DEFAULT: Options = Options { max_zoom: 16, radius: 40.0, extent: 512, min_points: 2 };

fn places() -> Vec<Feature<f64>> {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("testdata/supercluster/places.json");
    let json: Json = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    json["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let coords = &f["geometry"]["coordinates"];
            let mut feature = Feature::new(Point::new(coords[0].as_f64().unwrap(), coords[1].as_f64().unwrap()));
            for (key, value) in f["properties"].as_object().unwrap() {
                let value = match value {
                    Json::String(s) => Value::String(s.clone()),
                    Json::Number(n) if n.is_u64() => Value::Uint(n.as_u64().unwrap()),
                    Json::Number(n) => Value::Double(n.as_f64().unwrap()),
                    _ => continue,
                };
                feature.properties.insert(key.clone(), value);
            }
            feature
        })
        .collect()
}

fn uint(properties: &PropertyMap, key: &str) -> u64 {
    match properties.get(key) {
        Some(Value::Uint(u)) => *u,
        v => panic!("{key}: {v:?}"),
    }
}

fn name(feature: &Feature<f64>) -> &str {
    match feature.properties.get("name") {
        Some(Value::String(s)) => s,
        v => panic!("name: {v:?}"),
    }
}

fn is_cluster(properties: &PropertyMap) -> bool {
    matches!(properties.get("cluster"), Some(Value::Bool(true)))
}

fn point_count(tile: &TileFeatures) -> u64 {
    tile.iter().map(|f| if is_cluster(&f.properties) { uint(&f.properties, "point_count") } else { 1 }).sum()
}

/// Sums the points' `scalerank` into `sum`.
struct Sum;

impl Aggregate for Sum {
    fn map(&self, properties: &PropertyMap) -> PropertyMap {
        PropertyMap::from_iter([("sum".to_owned(), Value::Uint(uint(properties, "scalerank")))])
    }

    fn reduce(&self, accumulated: &mut PropertyMap, other: &PropertyMap) {
        let sum = uint(accumulated, "sum") + uint(other, "sum");
        accumulated.insert("sum".to_owned(), Value::Uint(sum));
    }
}

#[test]
fn clusters_every_point_once() {
    let clusters = Clusters::new(places(), DEFAULT, None).unwrap();
    let tile = clusters.tile(0, 0, 0);
    assert_eq!(tile.len(), 39);
    assert_eq!(point_count(&tile), 196);
}

#[test]
fn finds_a_clusters_children_leaves_and_expansion_zoom() {
    let clusters = Clusters::new(places(), DEFAULT, None).unwrap();

    let children = clusters.children(1).unwrap();
    let counts: Vec<_> =
        children.iter().map(|c| c.properties.get("point_count").map(|_| uint(&c.properties, "point_count"))).collect();
    assert_eq!(counts, [Some(6), Some(7), Some(2), None]);
    assert_eq!(name(&children[3]), "Bermuda Islands");

    let names: Vec<_> = clusters.children(994).unwrap().iter().map(|c| name(c).to_owned()).collect();
    assert_eq!(names, ["Cape Hope", "Point Barrow"]);

    assert_eq!(clusters.expansion_zoom(1), Ok(1));
    assert_eq!(clusters.expansion_zoom(33), Ok(1));
    assert_eq!(clusters.expansion_zoom(257), Ok(1));
    assert_eq!(clusters.expansion_zoom(994), Ok(2));

    let leaves = clusters.leaves(1, 10, 5).unwrap();
    assert_eq!(
        leaves.iter().map(name).collect::<Vec<_>>(),
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
fn leaves_page_through_every_point_of_a_cluster() {
    let clusters = Clusters::new(places(), DEFAULT, None).unwrap();
    let all = clusters.leaves(1, usize::MAX, 0).unwrap();
    let count = clusters
        .children(1)
        .unwrap()
        .iter()
        .map(|c| c.properties.get("point_count").map_or(1, |_| uint(&c.properties, "point_count")))
        .sum::<u64>();
    assert_eq!(all.len() as u64, count);
    let paged: Vec<_> = (0..all.len()).step_by(4).flat_map(|offset| clusters.leaves(1, 4, offset).unwrap()).collect();
    assert_eq!(paged, all);
    assert_eq!(clusters.leaves(1, 10, all.len()), Ok(Vec::new()));
    assert_eq!(clusters.leaves(1, 0, 0), Ok(Vec::new()));
}

#[test]
fn an_unknown_cluster_is_not_found() {
    let clusters = Clusters::new(places(), DEFAULT, None).unwrap();
    for id in [0, 7, 32, u32::MAX] {
        assert_eq!(clusters.children(id), Err(Error::NoSuchCluster), "{id}");
        assert_eq!(clusters.leaves(id, 0, 0), Err(Error::NoSuchCluster), "{id}");
        assert_eq!(clusters.expansion_zoom(id), Err(Error::NoSuchCluster), "{id}");
    }
}

#[test]
fn expansion_zoom_can_pass_the_max_zoom() {
    let options = Options { radius: 60.0, extent: 256, max_zoom: 4, ..DEFAULT };
    let clusters = Clusters::new(places(), options, None).unwrap();
    assert_eq!(clusters.expansion_zoom(357), Ok(5));
}

#[test]
fn aggregates_cluster_properties() {
    let clusters = Clusters::new(places(), DEFAULT, Some(&Sum)).unwrap();
    let tile = clusters.tile(0, 0, 0);
    assert_eq!(uint(&tile[0].properties, "sum"), 69);

    let options = Options { radius: 100.0, ..DEFAULT };
    let clusters = Clusters::new(places(), options, Some(&Sum)).unwrap();
    let sums = |tile: &TileFeatures| -> Vec<u64> {
        tile.iter().filter(|f| is_cluster(&f.properties)).map(|f| uint(&f.properties, "sum")).collect()
    };
    assert_eq!(sums(&clusters.tile(0, 0, 0)), [298, 122, 12, 36, 98, 7, 24, 8, 125, 98, 125, 12, 36, 8]);
    assert_eq!(sums(&clusters.tile(1, 0, 0)), [146, 84, 63, 23, 34, 12, 19, 29, 8, 8, 80, 35]);
}

#[test]
fn keeps_clusters_to_the_min_points() {
    let options = Options { min_points: 5, ..DEFAULT };
    let clusters = Clusters::new(places(), options, None).unwrap();
    let tile = clusters.tile(0, 0, 0);
    assert_eq!(tile.len(), 49);
    assert!(tile.iter().filter(|f| is_cluster(&f.properties)).all(|f| uint(&f.properties, "point_count") >= 5));
    assert_eq!(point_count(&tile), 195);
}

#[test]
fn keeps_a_points_feature() {
    let mut features = places();
    features[0].id = Identifier::String("first".to_owned());
    let clusters = Clusters::new(features.clone(), Options { min_points: usize::MAX, ..DEFAULT }, None).unwrap();
    let alone = clusters.tile(0, 0, 0);
    let first = alone.iter().find(|f| f.id == Identifier::String("first".to_owned())).unwrap();
    assert_eq!(*first.properties, features[0].properties);
}

#[test]
fn rejects_a_feature_that_isnt_a_point() {
    let mut features = places();
    features[3].geometry = None;
    assert_eq!(Clusters::new(features, DEFAULT, None).unwrap_err(), Error::NotAPoint(3));
}

#[test]
fn clamps_the_max_zoom() {
    let options = Options { max_zoom: u8::MAX, ..DEFAULT };
    let clusters = Clusters::new(places(), options, None).unwrap();
    assert_eq!(clusters.levels.len(), usize::from(MAX_ZOOM) + 2);
}

#[test]
fn clusters_no_points() {
    let clusters = Clusters::new(Vec::new(), DEFAULT, None).unwrap();
    assert_eq!(clusters.tile(0, 0, 0), TileFeatures::new());
}

#[test]
fn abbreviates_counts_as_gl_js_does() {
    assert_eq!(abbreviate(999), Value::Uint(999));
    for (count, text) in [(1000, "1k"), (1249, "1.2k"), (1250, "1.3k"), (9999, "10k"), (12_345, "12k")] {
        assert_eq!(abbreviate(count), Value::String(text.to_owned()));
    }
}

#[test]
fn a_cluster_that_takes_nothing_stays_as_it_is() {
    // Two points in one place cluster at the max zoom, and the cluster
    // stays the same below it.
    let mut features = places();
    features.truncate(1);
    features.push(features[0].clone());
    let clusters = Clusters::new(features, DEFAULT, None).unwrap();
    let tile = clusters.tile(0, 0, 0);
    let [cluster] = &tile[..] else { panic!("{tile:?}") };
    let id = uint(&cluster.properties, "cluster_id") as u32;
    assert_eq!(clusters.children(id).unwrap().len(), 2);
    assert_eq!(clusters.expansion_zoom(id), Ok(DEFAULT.max_zoom + 1));

    // A point alone isn't a cluster, whatever the min points.
    let mut features = places();
    features.truncate(1);
    let clusters = Clusters::new(features, Options { min_points: 1, ..DEFAULT }, None).unwrap();
    assert!(!is_cluster(&clusters.tile(0, 0, 0)[0].properties));
}
