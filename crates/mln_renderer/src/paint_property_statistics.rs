use std::marker::PhantomData;

use mln_util::Color;
use serde::{Deserialize, Serialize};

/// The value types a binder keeps statistics for: only `float` tracks a
/// maximum (the C++ `PaintPropertyStatistics<float>` specialization).
pub(crate) trait StatisticValue {
    /// `Some(value)` for `float`, `None` for the types whose statistics are
    /// the empty primary template.
    fn statistic(&self) -> Option<f32>;
}

impl StatisticValue for f32 {
    fn statistic(&self) -> Option<f32> {
        Some(*self)
    }
}

impl StatisticValue for Color {
    fn statistic(&self) -> Option<f32> {
        None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(bound = "")]
pub(crate) struct PaintPropertyStatistics<T> {
    max: Option<f32>,
    #[serde(skip)]
    _t: PhantomData<T>,
}

impl<T> Default for PaintPropertyStatistics<T> {
    fn default() -> Self {
        PaintPropertyStatistics { max: None, _t: PhantomData }
    }
}

impl<T: StatisticValue> PaintPropertyStatistics<T> {
    /// The largest value added; always `None` for value types other than
    /// `f32`.
    #[must_use]
    pub(crate) fn max(&self) -> Option<f32> {
        self.max
    }

    /// `add(value)`: `_max = _max ? std::max(*_max, value) : value`.
    pub(crate) fn add(&mut self, value: &T) {
        if let Some(value) = value.statistic() {
            self.max = Some(match self.max {
                // std::max(a, b) = (a < b) ? b : a
                Some(max) => {
                    if max < value {
                        value
                    } else {
                        max
                    }
                }
                None => value,
            });
        }
    }
}
