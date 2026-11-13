// Ports vendor/maplibre-native-base/deps/pixelmatch-cpp/test/test.cpp
//
// Reads the fixtures from the copy of the C++ repo's files in testdata/.

use std::path::{Path, PathBuf};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/vendor/maplibre-native-base/deps/pixelmatch-cpp/test/fixtures")
}

fn read_image(name: &str) -> Vec<u8> {
    let path = fixture_dir().join(format!("{name}.rgba"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[expect(clippy::too_many_arguments, reason = "a case's images, size, options and expected mismatch")]
fn diff_test(
    img_path1: &str,
    img_path2: &str,
    diff_path: &str,
    width: usize,
    height: usize,
    threshold: f64,
    include_aa: bool,
    expected_mismatch: u64,
) {
    println!("comparing {img_path1} to {img_path2}, threshold: {threshold}, includeAA: {}", u8::from(include_aa));

    let img1 = read_image(img_path1);
    let img2 = read_image(img_path2);
    let expected_diff = read_image(diff_path);

    let size = width * height * 4;
    let mut actual_diff = vec![0u8; size];

    assert_eq!(img1.len(), size);
    assert_eq!(img2.len(), size);
    assert_eq!(expected_diff.len(), size);

    let mismatch = pixelmatch::pixelmatch(&img1, &img2, width, height, Some(&mut actual_diff), threshold, include_aa);

    assert_eq!(mismatch, expected_mismatch);
    assert!(actual_diff == expected_diff, "diff image differs");
}

#[test]
fn main() {
    diff_test("1a", "1b", "1diff", 512, 256, 0.05, false, 143);
    diff_test("2a", "2b", "2diff", 256, 256, 0.05, false, 12439);
    diff_test("3a", "3b", "3diff", 512, 256, 0.05, false, 212);
    diff_test("4a", "4b", "4diff", 438, 412, 0.05, false, 36089);
}
