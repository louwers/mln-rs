// Reads the fixtures of vendor/earcut.hpp/test/fixtures/*.cpp in place.
// Each file defines one
//   static const Fixture<short|double> name("name", triangles, deviation,
//       libtessDeviation, { {{x,y},...}, ... });
// (test/fixtures/geometries.hpp); this parses that declaration.

use std::path::{Path, PathBuf};

/// The coordinate type `T` of `Fixture<T>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordType {
    Short,
    Double,
}

#[derive(Clone, Debug)]
pub struct Fixture {
    pub name: String,
    pub coord_type: CoordType,
    pub expected_triangles: usize,
    pub expected_earcut_deviation: f64,
    pub expected_libtess_deviation: f64,
    /// The polygon as `double`s (exact for `short` fixtures).
    pub polygon: Vec<Vec<(f64, f64)>>,
}

impl Fixture {
    /// The `Polygon<std::pair<short, short>>` of a `Fixture<short>`.
    #[must_use]
    pub fn short_polygon(&self) -> Vec<Vec<(i16, i16)>> {
        assert_eq!(self.coord_type, CoordType::Short);
        self.polygon.iter().map(|r| r.iter().map(|&(x, y)| (x as i16, y as i16)).collect()).collect()
    }
}

/// Directory of the fixtures in this repository: the copy of earcut.hpp's
/// in testdata/ (not in the published package).
#[must_use]
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/vendor/earcut.hpp/test/fixtures")
}

/// All fixtures of [`fixtures_dir`], sorted by name.
#[must_use]
pub fn load_all() -> Vec<Fixture> {
    load_all_in(&fixtures_dir())
}

/// All fixtures in `dir` (earcut.hpp's test/fixtures), sorted by name (as
/// `FixtureTester::collection()` sorts).
#[must_use]
pub fn load_all_in(dir: &Path) -> Vec<Fixture> {
    let mut fixtures: Vec<Fixture> = std::fs::read_dir(dir)
        .expect("earcut fixtures directory")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "cpp"))
        .map(|p| parse(&std::fs::read_to_string(&p).unwrap()))
        .collect();
    fixtures.sort_by(|a, b| a.name.cmp(&b.name));
    fixtures
}

/// The fixture `name` of [`fixtures_dir`].
#[must_use]
pub fn load(name: &str) -> Fixture {
    load_in(&fixtures_dir(), name)
}

/// The fixture `name` in `dir`.
#[must_use]
pub fn load_in(dir: &Path, name: &str) -> Fixture {
    let path = dir.join(format!("{name}.cpp"));
    parse(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

fn parse_number(s: &str) -> f64 {
    let s = s.trim();
    match s {
        "Infinity" => f64::INFINITY,
        _ => s.parse().unwrap_or_else(|_| panic!("bad number {s:?}")),
    }
}

pub fn parse(source: &str) -> Fixture {
    let start = source.find("static const Fixture<").expect("Fixture declaration");
    let rest = &source[start + "static const Fixture<".len()..];
    let close = rest.find('>').unwrap();
    let coord_type = match &rest[..close] {
        "short" => CoordType::Short,
        "double" => CoordType::Double,
        t => panic!("unknown fixture type {t}"),
    };
    let rest = &rest[close..];
    let open = rest.find('(').unwrap();
    let rest = &rest[open + 1..];
    let brace = rest.find('{').unwrap();
    let args: Vec<&str> = rest[..brace].split(',').map(str::trim).collect();
    let name = args[0].trim_matches('"').to_owned();
    let expected_triangles = args[1].parse().unwrap();
    let expected_earcut_deviation = parse_number(args[2]);
    let expected_libtess_deviation = parse_number(args[3]);

    // The polygon initializer: depth 1 = polygon, 2 = ring, 3 = point.
    let mut polygon: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut depth = 0;
    let mut number = String::new();
    let mut point: Vec<f64> = Vec::new();
    for c in rest[brace..].chars() {
        match c {
            '{' => {
                depth += 1;
                if depth == 2 {
                    polygon.push(Vec::new());
                }
            }
            '}' => {
                if depth == 3 {
                    point.push(parse_number(&number));
                    number.clear();
                    assert_eq!(point.len(), 2, "{name}: point with {} coordinates", point.len());
                    polygon.last_mut().unwrap().push((point[0], point[1]));
                    point.clear();
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 3 => {
                point.push(parse_number(&number));
                number.clear();
            }
            c if depth == 3 && !c.is_whitespace() => number.push(c),
            _ => {}
        }
    }
    assert_eq!(depth, 0, "{name}: unterminated polygon");

    Fixture { name, coord_type, expected_triangles, expected_earcut_deviation, expected_libtess_deviation, polygon }
}
