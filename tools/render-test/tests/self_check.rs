// Runs the whole shared render suite with each test's expected image as the
// rendered one: manifest, metadata and operation parsing, the image
// comparison and the reports. Every test that runs must pass.

mod suite;

#[test]
fn self_check() {
    assert_eq!(suite::run("self_check", &["--self-check", "--filter", "/render-tests/"], None), 0);
}
