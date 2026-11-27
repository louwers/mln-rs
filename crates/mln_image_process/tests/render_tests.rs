//! The layer type's render tests (render-tests/), which expect the same
//! images on every backend, through tools/render-test's runner with the
//! built-in types and this crate's. The tests run in this process, with the
//! report and the actual/diff images in `image_process_render_tests` in the
//! test output directory (`mln_headless::test_output_dir`).

use std::path::PathBuf;

use mln_engine::RegistryBuilder;
use render_test::render_test::{parse_arguments, run_render_tests_with};

#[test]
fn render_tests() {
    let package_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    let manifest = package_dir.join("render-tests/manifest.json");
    let output = mln_headless::test_output_dir(env!("CARGO_TARGET_TMPDIR")).join("image_process_render_tests");
    let argv = [
        "render-test",
        "--manifestPath",
        manifest.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--jobs",
        "1",
    ]
    .map(str::to_owned);
    let mut builder = RegistryBuilder::new();
    mln_builtins::register_builtins(&mut builder).unwrap();
    mln_image_process::register(&mut builder).unwrap();
    let factory = render_test::map::factory(builder.build());
    let code = parse_arguments(&argv)
        .map_or_else(|code| code, |arguments| run_render_tests_with(&arguments, Some(factory)).return_code);
    assert_eq!(code, 0);
}
