// The `std::map<double, std::unique_ptr<Expression>>` that Interpolate and
// Step hold, plus src/mln/style/expression/get_covering_stops.cpp.
//
// Keys are ordered with `<` like std::map<double>: equivalent keys (e.g. -0
// and +0) are one key.

use std::cmp::Ordering;

use crate::expression::Expression;
use crate::range::Range;

#[derive(Default)]
pub struct Stops {
    stops: Vec<(f64, Box<dyn Expression>)>,
}

impl Stops {
    #[must_use]
    pub fn new() -> Self {
        Stops { stops: Vec::new() }
    }

    /// The first index whose key is not less than `key`.
    #[must_use]
    pub(crate) fn lower_bound(&self, key: f64) -> usize {
        self.stops.partition_point(|(k, _)| *k < key)
    }

    /// The first index whose key is greater than `key`.
    #[must_use]
    pub(crate) fn upper_bound(&self, key: f64) -> usize {
        self.stops.partition_point(|(k, _)| key.partial_cmp(k) != Some(Ordering::Less))
    }

    /// Inserts `value` at `key` unless `key` is already present; returns
    /// whether it inserted.
    pub fn emplace(&mut self, key: f64, value: Box<dyn Expression>) -> bool {
        let i = self.lower_bound(key);
        if i < self.stops.len() && key.partial_cmp(&self.stops[i].0) != Some(Ordering::Less) {
            return false;
        }
        self.stops.insert(i, (key, value));
        true
    }

    /// `stops[key] = value`: inserts or replaces.
    pub(crate) fn insert_or_assign(&mut self, key: f64, value: Box<dyn Expression>) {
        let i = self.lower_bound(key);
        if i < self.stops.len() && key.partial_cmp(&self.stops[i].0) != Some(Ordering::Less) {
            self.stops[i].1 = value;
        } else {
            self.stops.insert(i, (key, value));
        }
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.stops.len()
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.stops.is_empty()
    }

    #[must_use]
    pub(crate) fn get(&self, i: usize) -> (f64, &dyn Expression) {
        let (k, v) = &self.stops[i];
        (*k, &**v)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (f64, &dyn Expression)> {
        self.stops.iter().map(|(k, v)| (*k, &**v))
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &Box<dyn Expression>> {
        self.stops.iter().map(|(_, v)| v)
    }

    /// Whether both have the same stops with equal outputs.
    #[must_use]
    pub(crate) fn children_equal(&self, rhs: &Stops) -> bool {
        if self.stops.len() != rhs.stops.len() {
            return false;
        }
        self.stops.iter().zip(&rhs.stops).all(|(l, r)| l.0 == r.0 && l.1.equals(&*r.1))
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
    if min_it != 0 && min_it != end && stops.get(min_it).0 > lower {
        min_it -= 1;
    }
    let last = stops.get(end - 1).0;
    Range::new(
        (if min_it == end { last } else { stops.get(min_it).0 }) as f32,
        (if max_it == end { last } else { stops.get(max_it).0 }) as f32,
    )
}
