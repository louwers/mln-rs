//! One scenario per thing a user waits for or feels, on a whole map over
//! the fixtures.

pub(crate) mod frame;
pub(crate) mod load;
pub(crate) mod navigate;
pub(crate) mod tile_server;
pub(crate) mod update;

pub(crate) use mln_bench_workload::paths::VIEW;
use mln_engine::camera::CameraOptions;

use crate::support::fixtures::{Fixtures, Tiles};

/// A camera at `zoom` over the center of the street tiles.
pub(crate) fn street_center(fixtures: &Fixtures, zoom: f64) -> CameraOptions {
    CameraOptions { center: Some(fixtures.area_center(Tiles::Street)), zoom: Some(zoom), ..Default::default() }
}
