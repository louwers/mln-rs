//! Notches: the values of the zoom, bearing and pitch where gestures report
//! feedback, from MapLibre Compose's `BearingHapticDetector`, for every
//! axis.

use crate::camera::CameraOptions;
use crate::gestures::input::GestureEvent;
use crate::gestures::options::{Axis, GestureOptions, Notch};

const AXES: [Axis; 3] = [Axis::Zoom, Axis::Bearing, Axis::Pitch];

/// The most notches of one [`Notch`] that a single sample checks: a move
/// that crosses more reports one event all the same.
const MAX_CROSSED: i64 = 64;
/// Feedback comes at most this often on an axis, in milliseconds; a notch
/// reached sooner still disarms, so it can't replay later.
const MIN_INTERVAL_MS: f64 = 50.0;

impl Axis {
    /// How near a notch a value reaches it, and how far it then moves away
    /// before the notch rearms, in the axis's unit. Compose's 1° and 2° for
    /// bearing, and pitch too; zoom's are provisional.
    fn capture_and_rearm(self) -> (f64, f64) {
        match self {
            Axis::Zoom => (0.02, 0.04),
            Axis::Bearing | Axis::Pitch => (1.0, 2.0),
        }
    }

    fn circular(self) -> bool {
        self == Axis::Bearing
    }

    fn value(self, camera: &CameraOptions) -> Option<f64> {
        match self {
            Axis::Zoom => camera.zoom,
            Axis::Bearing => camera.bearing,
            Axis::Pitch => camera.pitch,
        }
        .filter(|value| value.is_finite())
    }
}

/// One notch of a [`Notch`]: its index in the axis's list, and its step from
/// the offset (within a turn, for bearing).
type Key = (usize, i64);

/// The notches of one axis during one gesture.
#[derive(Debug, Default)]
struct AxisWatch {
    /// The value at the last sample.
    last: Option<f64>,
    /// Notches reached and not yet left far enough to rearm.
    disarmed: Vec<Key>,
    /// When the last event came, on the recognizer's clock.
    last_event: Option<f64>,
}

/// Watches the camera for notches from the start of a gesture until it, its
/// glide and its snap end.
#[derive(Debug, Default)]
pub(crate) struct NotchWatch {
    axes: [AxisWatch; 3],
}

impl NotchWatch {
    /// Starts watching where the camera is: a notch it starts at doesn't
    /// report until the value leaves it and comes back.
    pub(crate) fn new(camera: &CameraOptions, options: &GestureOptions) -> NotchWatch {
        let mut watch = NotchWatch::default();
        for (axis, state) in AXES.into_iter().zip(&mut watch.axes) {
            let Some(value) = axis.value(camera) else { continue };
            let (capture, _) = axis.capture_and_rearm();
            state.last = Some(value);
            state.disarmed = reached(axis, &options.axis(axis).notches, value, 0.0, capture).collect();
        }
        watch
    }

    /// Samples the camera at `now`, and pushes an event for each axis that
    /// reached an armed notch since the last sample.
    pub(crate) fn sample(
        &mut self,
        camera: &CameraOptions,
        options: &GestureOptions,
        now: f64,
        events: &mut Vec<GestureEvent>,
    ) {
        for (axis, state) in AXES.into_iter().zip(&mut self.axes) {
            let axis_options = options.axis(axis);
            let Some(to) = axis.value(camera) else { continue };
            let Some(from) = state.last.replace(to) else { continue };
            if !axis_options.enabled || axis_options.notches.is_empty() {
                continue;
            }
            let travel = if axis.circular() { bearing_delta(from, to) } else { to - from };
            if travel == 0.0 {
                continue;
            }
            let (capture, rearm) = axis.capture_and_rearm();
            let notches = &axis_options.notches;
            let mut first = None;
            for key in reached(axis, notches, from, travel, capture) {
                if state.disarmed.contains(&key) {
                    continue;
                }
                state.disarmed.push(key);
                first.get_or_insert(key);
            }
            state.disarmed.retain(|&key| {
                notches.get(key.0).is_some_and(|notch| distance(axis, notch_value(axis, notch, key.1), to) <= rearm)
            });
            let Some((index, step)) = first else { continue };
            if state.last_event.is_some_and(|last| now - last < MIN_INTERVAL_MS) {
                continue;
            }
            state.last_event = Some(now);
            let notch = notches[index];
            events.push(GestureEvent::Notch { axis, value: notch_value(axis, &notch, step), kind: notch.kind });
        }
    }
}

/// The notches that a move from `from` by `travel` reaches, within `capture`
/// of its path, in the order of the axis's list.
fn reached(axis: Axis, notches: &[Notch], from: f64, travel: f64, capture: f64) -> impl Iterator<Item = Key> {
    let (low, high) = (from + travel.min(0.0) - capture, from + travel.max(0.0) + capture);
    notches
        .iter()
        .enumerate()
        .flat_map(move |(index, notch)| steps(axis, notch, low, high).into_iter().map(move |step| (index, step)))
}

/// The steps of `notch` from `low` to `high`, at most [`MAX_CROSSED`]. On
/// the bearing's circle, the notches lie within one turn from the offset, as
/// snap targets do.
fn steps(axis: Axis, notch: &Notch, low: f64, high: f64) -> Vec<i64> {
    // Saturating float-to-int conversions keep a huge range finite.
    let between = |low: f64, high: f64, min: f64, max: f64| {
        let first = ((low - notch.offset) / notch.spacing).ceil().max(min) as i64;
        let end = ((high - notch.offset) / notch.spacing).floor().min(max) as i64;
        first..=end.min(first.saturating_add(MAX_CROSSED - 1))
    };
    if !axis.circular() {
        return between(low, high, f64::MIN, f64::MAX).collect();
    }
    // The steps within a turn, and the turns that the range spans.
    let last = ((360.0 / notch.spacing).ceil() - 1.0).max(0.0);
    let first_turn = ((low - notch.offset) / 360.0).floor() as i64;
    let last_turn = ((high - notch.offset) / 360.0).floor() as i64;
    let mut steps = Vec::new();
    for turn in first_turn..=last_turn {
        let shift = 360.0 * turn as f64;
        steps.extend(between(low - shift, high - shift, 0.0, last));
        if steps.len() >= MAX_CROSSED as usize {
            break;
        }
    }
    steps
}

/// The value of `notch`'s step; on the bearing's circle, in [0°, 360°).
fn notch_value(axis: Axis, notch: &Notch, step: i64) -> f64 {
    let value = notch.offset + step as f64 * notch.spacing;
    if axis.circular() { normalize_bearing(value) } else { value }
}

fn distance(axis: Axis, a: f64, b: f64) -> f64 {
    if axis.circular() { bearing_delta(a, b).abs() } else { (a - b).abs() }
}

fn normalize_bearing(bearing: f64) -> f64 {
    bearing.rem_euclid(360.0)
}

/// The shorter turn from `from` to `to`, in [-180°, 180°).
fn bearing_delta(from: f64, to: f64) -> f64 {
    (normalize_bearing(to) - normalize_bearing(from) + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
#[path = "notches_test.rs"]
mod notches_test;
