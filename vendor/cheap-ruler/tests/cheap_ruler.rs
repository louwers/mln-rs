// Ports vendor/maplibre-native-base/deps/cheap-ruler-cpp/test/cheap_ruler.cpp
// over test/fixtures/{lines,turf}.hpp (C++ initializers, parsed in place).
// `longitudeWrap` draws its east/west shifts from a fixed sequence instead
// of std::random_device (the assertions hold for any sequence).

use cheap_ruler::{Box, CheapRuler, Unit, remainder};
use geometry::{LineString, Point};

/// A brace initializer: nested lists of numbers.
#[derive(Debug)]
enum Node {
    Num(f64),
    List(Vec<Node>),
}

impl Node {
    fn list(&self) -> &[Node] {
        match self {
            Node::List(l) => l,
            Node::Num(_) => panic!("not a list"),
        }
    }
    fn num(&self) -> f64 {
        match self {
            Node::Num(n) => *n,
            Node::List(_) => panic!("not a number"),
        }
    }
    fn point(&self) -> Point<f64> {
        let l = self.list();
        Point::new(l[0].num(), l[1].num())
    }
}

fn fixture_file(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("vendor/maplibre-native-base/deps/cheap-ruler-cpp/test/fixtures")
            .join(name),
    )
    .unwrap()
}

/// The initializer of `static const ... <name> = { ... };`.
fn variable(source: &str, name: &str) -> Node {
    let marker = format!(" {name} =");
    let start = source.find(&marker).unwrap_or_else(|| panic!("{name} not found")) + marker.len();
    let mut stack: Vec<Vec<Node>> = Vec::new();
    let mut number = String::new();
    let flush = |number: &mut String, stack: &mut Vec<Vec<Node>>| {
        if !number.is_empty() {
            stack.last_mut().unwrap().push(Node::Num(number.parse().unwrap()));
            number.clear();
        }
    };
    for c in source[start..].chars() {
        match c {
            '{' => stack.push(Vec::new()),
            '}' => {
                flush(&mut number, &mut stack);
                let list = stack.pop().unwrap();
                match stack.last_mut() {
                    Some(parent) => parent.push(Node::List(list)),
                    None => return Node::List(list),
                }
            }
            ',' => flush(&mut number, &mut stack),
            c if c.is_whitespace() => {}
            c if !stack.is_empty() => number.push(c),
            _ => {}
        }
    }
    panic!("unterminated {name}");
}

struct Fixtures {
    points: LineString<f64>,
    lines: Vec<LineString<f64>>,
    turf_distance: Vec<f64>,
    turf_bearing: Vec<f64>,
    turf_destination: Vec<Point<f64>>,
    turf_line_distance: Vec<f64>,
    turf_area: Vec<f64>,
    turf_along_dist: Vec<f64>,
    turf_along: Vec<Point<f64>>,
    turf_line_slice: Vec<f64>,
    turf_buffer_point: Vec<Box>,
}

fn fixtures() -> Fixtures {
    let lines_hpp = fixture_file("lines.hpp");
    let turf_hpp = fixture_file("turf.hpp");
    let numbers = |name: &str| -> Vec<f64> { variable(&turf_hpp, name).list().iter().map(Node::num).collect() };
    let points = |n: Node| -> Vec<Point<f64>> { n.list().iter().map(Node::point).collect() };
    Fixtures {
        points: points(variable(&lines_hpp, "points")),
        lines: variable(&lines_hpp, "lines")
            .list()
            .iter()
            .map(|l| l.list().iter().map(Node::point).collect())
            .collect(),
        turf_distance: numbers("turf_distance"),
        turf_bearing: numbers("turf_bearing"),
        turf_destination: points(variable(&turf_hpp, "turf_destination")),
        turf_line_distance: numbers("turf_lineDistance"),
        turf_area: numbers("turf_area"),
        turf_along_dist: numbers("turf_along_dist"),
        turf_along: points(variable(&turf_hpp, "turf_along")),
        turf_line_slice: numbers("turf_lineSlice"),
        turf_buffer_point: variable(&turf_hpp, "turf_bufferPoint")
            .list()
            .iter()
            .map(|b| Box::new(b.list()[0].point(), b.list()[1].point()))
            .collect(),
    }
}

fn ruler() -> CheapRuler {
    CheapRuler::new(32.8351, Unit::Kilometers)
}

fn miles_ruler() -> CheapRuler {
    CheapRuler::new(32.8351, Unit::Miles)
}

fn assert_err(expected: f64, actual: f64, max_error: f64) {
    // Add a negligible fraction to make sure we don't divide by zero.
    let error = ((actual - expected) / (if expected == 0.0 { expected + 0.000_001 } else { expected })).abs();

    assert!(error <= max_error, "expected is {expected} but got {actual}");
}

fn p(x: f64, y: f64) -> Point<f64> {
    Point::new(x, y)
}

#[test]
fn distance() {
    let f = fixtures();
    for i in 0..f.points.len() - 1 {
        assert_err(f.turf_distance[i], ruler().distance(f.points[i], f.points[i + 1]), 0.003);
    }
}

#[test]
fn distance_in_miles() {
    let d = ruler().distance(p(30.5, 32.8351), p(30.51, 32.8451));
    let d2 = miles_ruler().distance(p(30.5, 32.8351), p(30.51, 32.8451));

    assert_err(d / d2, 1.609_344, 1e-12);
}

#[test]
fn bearing() {
    let f = fixtures();
    for i in 0..f.points.len() - 1 {
        assert_err(f.turf_bearing[i], ruler().bearing(f.points[i], f.points[i + 1]), 0.005);
    }
}

#[test]
fn destination() {
    let f = fixtures();
    for i in 0..f.points.len() {
        let bearing = (i % 360) as f64 - 180.0;
        let expected = f.turf_destination[i];
        let actual = ruler().destination(f.points[i], 1.0, bearing);

        assert_err(expected.x, actual.x, 1e-6); // longitude
        assert_err(expected.y, actual.y, 1e-6); // latitude
    }
}

#[test]
fn line_distance() {
    assert_err(0.0, ruler().line_distance(&vec![]), 0.0);

    let f = fixtures();
    for (i, line) in f.lines.iter().enumerate() {
        assert_err(f.turf_line_distance[i], ruler().line_distance(line), 0.003);
    }
}

#[test]
fn area() {
    let f = fixtures();
    let mut j = 0;
    for line in &f.lines {
        if line.len() < 3 {
            continue;
        }

        let mut ring = line.clone();
        ring.push(line[0]);

        let expected = f.turf_area[j];
        j += 1;
        assert_err(expected, ruler().area(&vec![ring]), 0.003);
    }
}

#[test]
fn along() {
    let actual = ruler().along(&vec![], 0.0);
    assert_err(0.0, actual.x, 0.0);
    assert_err(0.0, actual.y, 0.0);

    let f = fixtures();
    for (i, line) in f.lines.iter().enumerate() {
        let expected = f.turf_along[i];
        let actual = ruler().along(line, f.turf_along_dist[i]);

        assert_err(expected.x, actual.x, 1e-6); // along longitude
        assert_err(expected.y, actual.y, 1e-6); // along latitude
    }
}

#[test]
fn along_with_dist() {
    let f = fixtures();
    assert_eq!(ruler().along(&f.lines[0], -5.0), f.lines[0][0]);
}

#[test]
fn along_with_dist_greater_than_length() {
    let f = fixtures();
    assert_eq!(ruler().along(&f.lines[0], 1000.0), f.lines[0][f.lines[0].len() - 1]);
}

#[test]
fn point_on_line() {
    // not Turf comparison because pointOnLine is bugged
    // https://github.com/Turfjs/turf/issues/344
    let line = vec![p(-77.031_669, 38.878_605), p(-77.029_609, 38.881_946)];
    let result = ruler().point_on_line(&line, p(-77.034_076, 38.882_017));

    assert_err(result.0.x, -77.030_526_890_334_36, 1e-6);
    assert_err(result.0.y, 38.880_457_324_462_576, 1e-6);
    assert_eq!(result.1, 0); // index
    assert_err(result.2, 0.554_422_167_786_175_6, 1e-6); // t

    assert_eq!(ruler().point_on_line(&line, p(-80.0, 38.0)).2, 0.0, "t is not less than 0");
    assert_eq!(ruler().point_on_line(&line, p(-75.0, 38.0)).2, 1.0, "t is not bigger than 1");
}

#[test]
fn point_to_segment_distance() {
    let pt = p(-77.034_076, 38.882_017);
    let p0 = p(-77.031_669, 38.878_605);
    let p1 = p(-77.029_609, 38.881_946);
    assert_err(0.374_614_840_204_204_16, ruler().point_to_segment_distance(&pt, &p0, &p1), 1e-6);
}

#[test]
fn line_slice() {
    let f = fixtures();
    for (i, line) in f.lines.iter().enumerate() {
        let dist = ruler().line_distance(line);
        let start = ruler().along(line, dist * 0.3);
        let stop = ruler().along(line, dist * 0.7);
        let actual = ruler().line_distance(&ruler().line_slice(start, stop, line));

        // @todo Should update turf_lineSlice and revert maxError back.
        assert_err(f.turf_line_slice[i], actual, 1e-4);
    }
}

#[test]
fn line_slice_along() {
    let empty: LineString<f64> = vec![];
    assert_err(ruler().line_distance(&empty), ruler().line_distance(&ruler().line_slice_along(0.0, 0.0, &empty)), 0.0);

    let f = fixtures();
    for (i, line) in f.lines.iter().enumerate() {
        if i == 46 {
            // skip due to Turf bug https://github.com/Turfjs/turf/issues/351
            continue;
        }

        let dist = ruler().line_distance(line);
        let actual = ruler().line_distance(&ruler().line_slice_along(dist * 0.3, dist * 0.7, line));

        // @todo Should update turf_lineSlice and revert maxError back.
        assert_err(f.turf_line_slice[i], actual, 1e-4);
    }
}

#[test]
fn line_slice_reverse() {
    let f = fixtures();
    let line = &f.lines[0];
    let dist = ruler().line_distance(line);
    let start = ruler().along(line, dist * 0.7);
    let stop = ruler().along(line, dist * 0.3);
    let actual = ruler().line_distance(&ruler().line_slice(start, stop, line));

    assert_err(0.018_676_476_689_649_835, actual, 1e-6);
}

#[test]
fn buffer_point() {
    let f = fixtures();
    for (i, point) in f.points.iter().enumerate() {
        let expected = f.turf_buffer_point[i];
        let actual = miles_ruler().buffer_point(*point, 0.1);

        assert_err(expected.min.x, actual.min.x, 2e-7);
        assert_err(expected.min.x, actual.min.x, 2e-7);
        assert_err(expected.max.y, actual.max.y, 2e-7);
        assert_err(expected.max.y, actual.max.y, 2e-7);
    }
}

#[test]
fn buffer_bbox() {
    let bbox = Box::new(p(30.0, 38.0), p(40.0, 39.0));
    let bbox2 = ruler().buffer_bbox(bbox, 1.0);

    assert_err(bbox2.min.x, 29.989_319_515_875_376, 1e-6);
    assert_err(bbox2.min.y, 37.990_982_712_257_11, 1e-6);
    assert_err(bbox2.max.x, 40.010_680_484_124_62, 1e-6);
    assert_err(bbox2.max.y, 39.009_017_287_742_89, 1e-6);
}

#[test]
fn inside_bbox() {
    let bbox = Box::new(p(30.0, 38.0), p(40.0, 39.0));

    assert!(CheapRuler::inside_bbox(p(35.0, 38.5), bbox));
    assert!(!CheapRuler::inside_bbox(p(45.0, 45.0), bbox));
}

#[test]
fn from_tile() {
    let ruler1 = CheapRuler::new(50.5, Unit::Kilometers);
    let ruler2 = CheapRuler::from_tile(11041, 15);

    let p1 = p(30.5, 50.5);
    let p2 = p(30.51, 50.51);

    assert_err(ruler1.distance(p1, p2), ruler2.distance(p1, p2), 2e-5);
}

#[test]
fn longitude_wrap() {
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut bernoulli = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed & 1 == 1
    };

    let r = CheapRuler::new(50.5, Unit::Kilometers);
    let mut ring = Vec::new();
    let mut line = Vec::new();
    let origin = p(0.0, 50.5); // Greenwich
    let rad = 1000.0;
    // construct a regular dodecagon
    let mut i = -180;
    while i <= 180 {
        let mut pt = r.destination(origin, rad, f64::from(i));
        // shift randomly east/west to the international date line
        pt.x += if bernoulli() { 180.0 } else { -180.0 };
        ring.push(pt);
        line.push(pt);
        i += 30;
    }
    let perimeter = r.line_distance(&line);
    let a = r.area(&vec![ring]);
    // cheap_ruler does planar calculations, so the perimeter and area of a
    // planar regular dodecagon with circumradius rad are used in these
    // checks.
    assert_err(12.0 * rad / (2.0 + 3.0f64.sqrt()).sqrt(), perimeter, 1e-12);
    assert_err(3.0 * rad * rad, a, 1e-12);
    for j in 1..line.len() {
        let azi = r.bearing(line[j - 1], line[j]);
        // offset expect and actual by 1 to make err criterion absolute
        assert_err(1.0, remainder(270.0 - 15.0 + 30.0 * j as f64 - azi, 360.0) + 1.0, 1e-12);
    }
}
