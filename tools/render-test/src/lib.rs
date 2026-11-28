//! The render-test runner: renders the style tests of the render suite and
//! compares them with their expected images and results.
//!
//! The runner core reaches the map through [`runner::TestRunnerImpl`],
//! implemented over the real map in [`map`], which draws it with
//! `mln_headless::HeadlessFrontend`. `--self-check` runs the full comparison and
//! reporting path without a map, with each test's expected image as the
//! rendered one.

use std::sync::Arc;

use mln_engine::{Registry, RegistryBuilder};

pub mod file_source;
mod filesystem;
mod jobs;
mod log;
mod manifest_parser;
pub mod map;
mod metadata;
mod national_grid;
mod parser;
pub mod render_test;
pub mod runner;
mod style_ops;
mod test_runner_common;
mod watchdog;

pub use render_test::run_render_tests;

/// The registrations that the render tests' maps use: the built-ins, and
/// EPSG:27700, the British National Grid, which the
/// `projection/national-grid` tests draw in, as MapLibre Native's runner
/// registers it.
#[must_use]
pub fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    let registered = mln_builtins::register_builtins(&mut builder)
        .and_then(|()| builder.add_projection(national_grid::national_grid("EPSG:27700")));
    debug_assert_eq!(registered, Ok(()), "the built-ins and the National Grid register into an empty builder");
    builder.build()
}

#[cfg(test)]
mod tests;
