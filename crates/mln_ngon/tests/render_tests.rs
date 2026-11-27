//! The layer type's render and query tests (render-tests/), which expect
//! the same images on every backend, through tools/render-test's runner
//! with the built-in types and this one. The tests run in this process,
//! with the report and the actual/diff images in `ngon_<test>` in the test
//! output directory (`mln_headless::test_output_dir`).

use std::path::PathBuf;

use mln_engine::RegistryBuilder;
use render_test::render_test::{parse_arguments, run_render_tests_with};

/// The runner's exit code for the tests, with `args` after the manifest,
/// output and jobs.
fn run(name: &str, args: &[&str]) -> i32 {
    let package_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    let manifest = package_dir.join("render-tests/manifest.json");
    let output = mln_headless::test_output_dir(env!("CARGO_TARGET_TMPDIR")).join(name);
    let mut argv = vec![
        "render-test".to_owned(),
        "--manifestPath".to_owned(),
        manifest.to_str().unwrap().to_owned(),
        "--output".to_owned(),
        output.to_str().unwrap().to_owned(),
        "--jobs".to_owned(),
        "1".to_owned(),
    ];
    argv.extend(args.iter().map(ToString::to_string));
    let mut builder = RegistryBuilder::new();
    mln_builtins::register_builtins(&mut builder).unwrap();
    mln_ngon::register(&mut builder).unwrap();
    match parse_arguments(&argv) {
        Ok(arguments) => {
            run_render_tests_with(&arguments, Some(render_test::map::factory(builder.build()))).return_code
        }
        Err(code) => code,
    }
}

#[test]
fn render_tests() {
    assert_eq!(run("ngon_render_tests", &[]), 0);
}

/// The same tests with the tile workers in two worker instances that the
/// map reaches only through the worker codec: every bucket crosses as bytes.
#[test]
fn render_tests_through_the_codec() {
    assert_eq!(run("ngon_render_tests_through_the_codec", &["--tileWorkerCodec", "2"]), 0);
}
