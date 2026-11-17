//! A scenario's run as its process reports it, and how numbers print.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::environment::Environment;
use crate::scenario::{Kind, Metric, Unit};

/// What one process reports for one scenario.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Run {
    pub(crate) scenario: String,
    pub(crate) kind: Kind,
    /// The harness id of the binary that ran it (build.rs).
    pub(crate) harness: String,
    pub(crate) environment: Environment,
    pub(crate) metrics: Vec<Metric>,
}

impl Run {
    pub(crate) fn metric(&self, name: &str) -> Option<&Metric> {
        self.metrics.iter().find(|metric| metric.name == name)
    }
}

/// `value` in `unit`, scaled for reading: `12.3 M`, `4.5 MiB`, `6.78 ms`.
pub(crate) fn format_value(unit: Unit, value: f64) -> String {
    let sign = if value < 0.0 { "-" } else { "" };
    let magnitude = value.abs();
    match unit {
        Unit::Count => {
            let (scaled, suffix) = if magnitude >= 1e9 {
                (magnitude / 1e9, "G")
            } else if magnitude >= 1e6 {
                (magnitude / 1e6, "M")
            } else if magnitude >= 1e4 {
                (magnitude / 1e3, "k")
            } else {
                return format!("{sign}{magnitude}");
            };
            format!("{sign}{} {suffix}", three_digits(scaled))
        }
        Unit::Bytes => {
            let (scaled, suffix) = if magnitude >= 1024.0 * 1024.0 * 1024.0 {
                (magnitude / (1024.0 * 1024.0 * 1024.0), "GiB")
            } else if magnitude >= 1024.0 * 1024.0 {
                (magnitude / (1024.0 * 1024.0), "MiB")
            } else if magnitude >= 1024.0 {
                (magnitude / 1024.0, "KiB")
            } else {
                return format!("{sign}{magnitude} B");
            };
            format!("{sign}{} {suffix}", three_digits(scaled))
        }
        Unit::Ms => format!("{sign}{} ms", three_digits(magnitude)),
    }
}

/// Three significant digits, or the whole number when it has more.
fn three_digits(value: f64) -> String {
    if value >= 100.0 {
        format!("{value:.0}")
    } else if value >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

/// A plain-text table: the first column left-aligned, the others right.
pub(crate) fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| rows.iter().filter_map(|row| row.get(column)).map(|cell| cell.chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for row in rows {
        let mut line = String::from("  ");
        for (column, cell) in row.iter().enumerate() {
            if column == 0 {
                let _ = write!(line, "{cell:<width$}", width = widths[0]);
            } else {
                let _ = write!(line, "  {cell:>width$}", width = widths[column]);
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// `mln-bench run`'s report of `runs`, for reading.
pub(crate) fn runs_text(runs: &[Run]) -> String {
    let mut out = String::new();
    if let Some(first) = runs.first() {
        let _ = writeln!(out, "machine: {}\n", Environment { gpu: None, fixtures: None, ..first.environment.clone() });
    }
    for run in runs {
        let _ = writeln!(out, "{}", run.scenario);
        if let Some(gpu) = &run.environment.gpu {
            let software = if gpu.software { ", CPU rasterizer" } else { "" };
            let _ = writeln!(out, "  gpu: {} ({}{software})", gpu.name, gpu.backend);
        }
        let rows: Vec<Vec<String>> = run
            .metrics
            .iter()
            .map(|metric| vec![metric.name.clone(), format_value(metric.unit, metric.value)])
            .collect();
        out.push_str(&table(&rows));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_values_for_reading() {
        assert_eq!(format_value(Unit::Count, 812_345_678.0), "812 M");
        assert_eq!(format_value(Unit::Count, 12_345.0), "12.3 k");
        assert_eq!(format_value(Unit::Count, 42.0), "42");
        assert_eq!(format_value(Unit::Bytes, 3.5 * 1024.0 * 1024.0), "3.50 MiB");
        assert_eq!(format_value(Unit::Bytes, -2048.0), "-2.00 KiB");
        assert_eq!(format_value(Unit::Ms, 1.234), "1.23 ms");
    }
}
