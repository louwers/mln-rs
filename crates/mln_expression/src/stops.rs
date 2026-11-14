// The stops of an `interpolate` or a `step`, plus the stops that cover a
// zoom range.
//
// Keys are ordered with `<`: equivalent keys (-0 and +0) are one key.

use std::cmp::Ordering;

use crate::expression::Expr;
use crate::range::Range;

/// The stops of a curve: outputs by input key, in key order. The keys are
/// kept apart from the outputs, so that finding a stop reads only keys.
#[derive(Clone, Default, PartialEq)]
pub struct Stops {
    keys: Vec<f64>,
    outputs: Vec<Expr>,
}

impl Stops {
    #[must_use]
    pub fn new() -> Self {
        Stops::default()
    }

    /// The first index whose key is not less than `key`.
    #[must_use]
    pub(crate) fn lower_bound(&self, key: f64) -> usize {
        self.keys.partition_point(|k| *k < key)
    }

    /// The first index whose key is greater than `key`.
    #[must_use]
    pub(crate) fn upper_bound(&self, key: f64) -> usize {
        self.keys.partition_point(|k| key.partial_cmp(k) != Some(Ordering::Less))
    }

    /// Inserts `value` at `key` unless `key` is already present; returns
    /// whether it inserted.
    pub fn emplace(&mut self, key: f64, value: Expr) -> bool {
        let i = self.lower_bound(key);
        if i < self.keys.len() && key.partial_cmp(&self.keys[i]) != Some(Ordering::Less) {
            return false;
        }
        self.keys.insert(i, key);
        self.outputs.insert(i, value);
        true
    }

    /// Inserts `value` at `key`, or replaces the output there.
    pub(crate) fn insert_or_assign(&mut self, key: f64, value: Expr) {
        let i = self.lower_bound(key);
        if i < self.keys.len() && key.partial_cmp(&self.keys[i]) != Some(Ordering::Less) {
            self.outputs[i] = value;
        } else {
            self.keys.insert(i, key);
            self.outputs.insert(i, value);
        }
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    #[must_use]
    pub(crate) fn key(&self, i: usize) -> f64 {
        self.keys[i]
    }

    #[must_use]
    pub(crate) fn output(&self, i: usize) -> &Expr {
        &self.outputs[i]
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (f64, &Expr)> {
        self.keys.iter().copied().zip(&self.outputs)
    }

    pub(crate) fn values(&self) -> std::slice::Iter<'_, Expr> {
        self.outputs.iter()
    }
}

/// The smallest range of stops that covers the interval from `lower` to
/// `upper`.
#[must_use]
pub(crate) fn get_covering_stops(stops: &Stops, lower: f64, upper: f64) -> Range<f32> {
    debug_assert!(!stops.is_empty());
    if stops.is_empty() {
        return Range::new(f32::NAN, f32::NAN);
    }

    let mut min_it = stops.lower_bound(lower);
    let max_it = stops.lower_bound(upper);
    let end = stops.len();

    // lower_bound yields first element >= lowerZoom, but we want the *last*
    // element <= lowerZoom, so if we found a stop > lowerZoom, back up by one.
    if min_it != 0 && min_it != end && stops.key(min_it) > lower {
        min_it -= 1;
    }
    let last = stops.key(end - 1);
    Range::new(
        (if min_it == end { last } else { stops.key(min_it) }) as f32,
        (if max_it == end { last } else { stops.key(max_it) }) as f32,
    )
}
