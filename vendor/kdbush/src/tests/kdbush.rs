// Ports vendor/kdbush.hpp/test.cpp (testRange, testRadius, testEmpty). The
// points and expected ids are inline in test.cpp; they are read from it.

use crate::KDBush;

type TPoint = (i32, i32);

fn test_source() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/vendor/kdbush.hpp/test.cpp"),
    )
    .unwrap()
}

/// The `{ ... }` initializer after `marker`.
fn initializer<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source.find(marker).unwrap_or_else(|| panic!("{marker} not in test.cpp")) + marker.len();
    let rest = &source[start..];
    let open = rest.find('{').unwrap();
    let mut depth = 0;
    for (i, c) in rest.char_indices().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &rest[open + 1..i];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated initializer");
}

fn numbers(s: &str) -> Vec<i64> {
    s.split(|c: char| !(c.is_ascii_digit() || c == '-')).filter(|t| !t.is_empty()).map(|t| t.parse().unwrap()).collect()
}

fn points() -> Vec<TPoint> {
    let n = numbers(initializer(&test_source(), "static std::vector<TPoint> points ="));
    n.chunks(2).map(|c| (c[0] as i32, c[1] as i32)).collect()
}

/// The `expectedIds` of the test function `name`.
fn expected_ids(name: &str) -> Vec<usize> {
    let source = test_source();
    let body = &source[source.find(name).unwrap()..];
    numbers(initializer(body, "TIds expectedIds =")).into_iter().map(|n| n as usize).collect()
}

/// `std::equal(expected.begin(), expected.end(), result.begin())`
fn starts_with(result: &[usize], expected: &[usize]) -> bool {
    result.len() >= expected.len() && result[..expected.len()] == *expected
}

#[test]
fn test_range() {
    let index: KDBush<i32, usize> = KDBush::from_points(&points(), 10);
    let expected = expected_ids("testRange");
    let mut result = Vec::new();
    index.range(20, 30, 50, 70, &mut |id| result.push(id));

    assert!(starts_with(&result, &expected), "{result:?} vs {expected:?}");
}

#[test]
fn test_radius() {
    let index: KDBush<i32, usize> = KDBush::from_points(&points(), 10);
    let expected = expected_ids("testRadius");
    let mut result = Vec::new();
    index.within(50, 50, 20, &mut |id| result.push(id));

    assert!(starts_with(&result, &expected), "{result:?} vs {expected:?}");
}

#[test]
fn test_empty() {
    let empty_points: Vec<TPoint> = Vec::new();
    let index: KDBush<i32, usize> = KDBush::from_points(&empty_points, crate::DEFAULT_NODE_SIZE);
    // test no crash when using empty index
    let mut result: Vec<usize> = Vec::new();
    index.within(50, 50, 20, &mut |id| result.push(id));
    assert_eq!(result, Vec::<usize>::new());
    index.range(20, 30, 50, 70, &mut |id| result.push(id));
    assert_eq!(result, Vec::<usize>::new());
}
