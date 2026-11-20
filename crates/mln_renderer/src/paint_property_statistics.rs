use std::marker::PhantomData;

use mln_util::Color;

/// The value types a binder keeps statistics of: only `f32` tracks a
/// maximum.
pub trait StatisticValue {
    /// The value that the statistics track: `Some(value)` for `f32`, `None`
    /// for the types they don't track.
    fn statistic(&self) -> Option<f32>;
}

impl StatisticValue for f32 {
    #[inline]
    fn statistic(&self) -> Option<f32> {
        Some(*self)
    }
}

impl StatisticValue for Color {
    #[inline]
    fn statistic(&self) -> Option<f32> {
        None
    }
}

#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) struct PaintPropertyStatistics<T> {
    max: Option<f32>,
    #[wire(skip)]
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
