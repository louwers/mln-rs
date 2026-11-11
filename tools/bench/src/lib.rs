//! The benchmarks of mln-rs (README.md).
//!
//! `mln-bench run` runs the [`cases`] in one process and reports one value
//! per metric and case: a median over the case's iterations, or a
//! percentile over its frames. `mln-bench compare` ([`compare`]) runs both
//! sides in alternating processes and tests each case's lead metrics, the
//! ones a change is judged by; the others are context.

pub mod cases;
pub mod compare;
pub mod latency;
pub mod meter;
pub mod scene;
pub mod stats;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One metric of a case. Every metric is lower-is-better.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub value: f64,
    pub lead: bool,
}

/// A case's metrics, in the order the report shows them.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metrics(pub Vec<Metric>);

impl Metrics {
    pub fn lead(&mut self, name: &str, value: f64) {
        self.0.push(Metric { name: name.to_string(), value, lead: true });
    }

    pub fn other(&mut self, name: &str, value: f64) {
        self.0.push(Metric { name: name.to_string(), value, lead: false });
    }

    /// Keeps `name` as context only, for a case where it isn't the point.
    pub fn demote(&mut self, name: &str) {
        for metric in self.0.iter_mut().filter(|metric| metric.name == name) {
            metric.lead = false;
        }
    }

    pub fn extend(&mut self, other: Metrics) {
        self.0.extend(other.0);
    }

    pub fn get(&self, name: &str) -> Option<&Metric> {
        self.0.iter().find(|metric| metric.name == name)
    }
}

/// What one `mln-bench run` process reports.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub environment: Environment,
    pub cases: Vec<CaseReport>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaseReport {
    pub name: String,
    pub metrics: Metrics,
}

/// What the numbers were measured on. A comparison gives no verdicts when
/// the two sides' GPUs differ.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    /// The GPU's name and backend, and with WebGPU its driver, as wgpu
    /// reports them.
    pub gpu: String,
    /// A software rasterizer (lavapipe, llvmpipe, SwiftShader, WARP), which
    /// measures something else than a GPU.
    pub software_gpu: bool,
    pub os: String,
    /// The basemap fixture's version (fixtures.json).
    pub fixtures: String,
}

/// What every case gets.
#[derive(Clone, Debug)]
pub struct Context {
    /// The basemap fixture that `fixtures.py` prepared.
    pub fixtures: PathBuf,
    /// Multiplies every case's iteration count.
    pub scale: f64,
    /// Where the cases write their last frames, for `compare` to diff.
    pub frames: Option<PathBuf>,
}

impl Context {
    /// `count` iterations, scaled, and at least one.
    pub fn iterations(&self, count: usize) -> usize {
        ((count as f64 * self.scale).round() as usize).max(1)
    }
}
