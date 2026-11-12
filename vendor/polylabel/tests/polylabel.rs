// Ports vendor/polylabel/test/test.cpp over test/fixtures, read in place.

use geometry::{Point, Polygon};
use polylabel::polylabel;

fn fixture(name: &str) -> Polygon<f64> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/vendor/polylabel/test/fixtures").join(name);
    let d: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();

    d.as_array()
        .unwrap()
        .iter()
        .map(|ring| {
            ring.as_array()
                .unwrap()
                .iter()
                .map(|p| Point::new(p[0].as_f64().unwrap(), p[1].as_f64().unwrap()))
                .collect()
        })
        .collect()
}

#[test]
fn polylabel_test() {
    let water1 = fixture("water1.json");
    let water2 = fixture("water2.json");

    // finds pole of inaccessibility for water1 and precision 1
    assert_eq!(polylabel(&water1, 1.0).unwrap(), Point::new(3_865.850_097_656_25, 2_124.878_417_968_75));

    // finds pole of inaccessibility for water1 and precision 50
    assert_eq!(polylabel(&water1, 50.0).unwrap(), Point::new(3_854.296_875, 2_123.828_125));

    // finds pole of inaccessibility for water2 and default precision 1
    assert_eq!(polylabel(&water2, 1.0).unwrap(), Point::new(3263.5, 3263.5));

    // works on degenerate polygons
    let degenerate1: Polygon<f64> =
        vec![vec![Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(2.0, 0.0), Point::new(0.0, 0.0)]];
    assert_eq!(polylabel(&degenerate1, 1.0).unwrap(), Point::new(0.0, 0.0));
    let degenerate2: Polygon<f64> = vec![vec![
        Point::new(0.0, 0.0),
        Point::new(1.0, 0.0),
        Point::new(1.0, 1.0),
        Point::new(1.0, 0.0),
        Point::new(0.0, 0.0),
    ]];
    assert_eq!(polylabel(&degenerate2, 1.0).unwrap(), Point::new(0.0, 0.0));
}
