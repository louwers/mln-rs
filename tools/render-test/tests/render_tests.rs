// Every render and query test of the shared suite on the Rust renderer. The
// port's ignore file (ignores/rust-port.json) lists what is expected to fail.

mod suite;

#[test]
fn render_tests() {
    assert_eq!(suite::run("render_tests", &[], Some(render_test::map::factory())), 0);
}

/// The same tests with the tile workers in two worker instances that the map
/// reaches only through the worker codec, as in the browser build: every
/// message, bucket and tile crosses as bytes.
#[test]
fn render_tests_through_the_codec() {
    let factory = Some(render_test::map::factory());
    assert_eq!(suite::run("render_tests_through_the_codec", &["--tileWorkerCodec", "2"], factory), 0);
}
