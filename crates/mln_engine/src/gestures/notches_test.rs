// Ported from MapLibre Compose's `BearingHapticDetectorTest`, with the same
// rules for zoom and pitch.

use super::*;

/// A watch over the options' notches, fed camera values one axis at a time.
struct Probe {
    options: GestureOptions,
    axis: Axis,
    watch: NotchWatch,
}

impl Probe {
    fn new(axis: Axis, notches: &[Notch], start: f64) -> Probe {
        let mut options = GestureOptions::default();
        for axis in AXES {
            options.axis_mut(axis).notches.clear();
        }
        options.axis_mut(axis).notches = notches.to_vec();
        let watch = NotchWatch::new(&camera(axis, start), &options);
        Probe { options, axis, watch }
    }

    /// The notch, and its kind, that moving to `to` at `now` reports.
    fn to(&mut self, to: f64, now: f64) -> Option<(f64, u32)> {
        let mut events = Vec::new();
        self.watch.sample(&camera(self.axis, to), &self.options, now, &mut events);
        assert!(events.len() <= 1, "{events:?}");
        events.first().map(|event| match *event {
            GestureEvent::Notch { axis, value, kind } => {
                assert_eq!(axis, self.axis);
                (value, kind)
            }
            _ => panic!("{event:?}"),
        })
    }
}

impl GestureOptions {
    fn axis_mut(&mut self, axis: Axis) -> &mut crate::gestures::AxisOptions {
        match axis {
            Axis::Zoom => &mut self.zoom,
            Axis::Bearing => &mut self.bearing,
            Axis::Pitch => &mut self.pitch,
        }
    }
}

fn camera(axis: Axis, value: f64) -> CameraOptions {
    let mut camera = CameraOptions { zoom: Some(10.0), bearing: Some(90.0), pitch: Some(30.0), ..Default::default() };
    match axis {
        Axis::Zoom => camera.zoom = Some(value),
        Axis::Bearing => camera.bearing = Some(value),
        Axis::Pitch => camera.pitch = Some(value),
    }
    camera
}

const NORTH: [Notch; 1] = [Notch::NORTH];

#[test]
fn starting_at_a_notch_is_silent_and_jitter_requires_leaving_before_reentry() {
    let mut probe = Probe::new(Axis::Bearing, &NORTH, 0.0);
    assert_eq!(probe.to(0.5, 0.0), None);
    assert_eq!(probe.to(-0.5, 100.0), None);
    assert_eq!(probe.to(-3.0, 200.0), None);
    assert_eq!(probe.to(-0.5, 300.0), Some((0.0, 0)));
    assert_eq!(probe.to(0.5, 400.0), None);
    assert_eq!(probe.to(-0.5, 500.0), None);
}

#[test]
fn crossings_between_samples_report_in_both_directions_across_north() {
    let mut probe = Probe::new(Axis::Bearing, &NORTH, 355.0);
    assert_eq!(probe.to(5.0, 0.0), Some((0.0, 0)));
    assert_eq!(probe.to(355.0, 100.0), Some((0.0, 0)));
    // Bearings past a turn are the same notches.
    assert_eq!(probe.to(725.0, 200.0), Some((0.0, 0)));
}

#[test]
fn offsets_and_layered_notches_report_the_first_listed_kind() {
    let notches = [
        Notch { spacing: 360.0, offset: 32.0, kind: 1 },
        Notch { spacing: 90.0, offset: 32.0, kind: 2 },
        Notch { spacing: 15.0, offset: 32.0, kind: 3 },
    ];
    let mut probe = Probe::new(Axis::Bearing, &notches, 28.0);
    assert_eq!(probe.to(34.0, 0.0), Some((32.0, 1)));
    assert_eq!(probe.to(49.0, 100.0), Some((47.0, 3)));
    assert_eq!(probe.to(124.0, 200.0), Some((122.0, 2)));
}

#[test]
fn fast_moves_coalesce_crossings_without_replaying_suppressed_ones() {
    let mut probe = Probe::new(Axis::Bearing, &NORTH, -5.0);
    assert_eq!(probe.to(5.0, 0.0), Some((0.0, 0)));
    assert_eq!(probe.to(0.5, 10.0), None);
    assert_eq!(probe.to(-0.5, 100.0), None);
    assert_eq!(probe.to(-5.0, 200.0), None);
    assert_eq!(probe.to(5.0, 300.0), Some((0.0, 0)));
}

#[test]
fn bearing_notches_lie_every_spacing_from_the_offset_within_one_turn() {
    // From -358°: 2°, 92°, 182° and 272°.
    let mut probe = Probe::new(Axis::Bearing, &[Notch { spacing: 90.0, offset: -358.0, kind: 0 }], 80.0);
    assert_eq!(probe.to(100.0, 0.0), Some((92.0, 0)));
    // A spacing that doesn't divide the circle: 0° and 270° only, so 180°
    // and 540° aren't notches.
    let mut probe = Probe::new(Axis::Bearing, &[Notch { spacing: 270.0, offset: 0.0, kind: 0 }], 170.0);
    assert_eq!(probe.to(190.0, 0.0), None);
    assert_eq!(probe.to(275.0, 100.0), Some((270.0, 0)));
}

#[test]
fn zoom_and_pitch_report_their_notches_without_wrapping() {
    let levels = [Notch { spacing: 1.0, offset: 0.0, kind: 4 }];
    let mut probe = Probe::new(Axis::Zoom, &levels, 12.5);
    assert_eq!(probe.to(13.2, 0.0), Some((13.0, 4)));
    // Back across it, from more than 0.04 levels away, reports it again.
    assert_eq!(probe.to(13.0, 100.0), Some((13.0, 4)));
    assert_eq!(probe.to(13.03, 200.0), None);
    assert_eq!(probe.to(12.99, 300.0), None);
    assert_eq!(probe.to(12.0, 400.0), Some((12.0, 4)));
    // Within 0.02 levels reaches a notch.
    assert_eq!(probe.to(11.5, 500.0), None);
    assert_eq!(probe.to(11.015, 600.0), Some((11.0, 4)));

    let flat = [Notch { spacing: 90.0, offset: 0.0, kind: 5 }];
    let mut probe = Probe::new(Axis::Pitch, &flat, 20.0);
    assert_eq!(probe.to(0.5, 0.0), Some((0.0, 5)));
    let mut probe = Probe::new(Axis::Pitch, &flat, 300.0);
    assert_eq!(probe.to(370.0, 0.0), Some((360.0, 5)));
}

#[test]
fn tiny_spacings_stay_bounded() {
    // A tiny spacing checks a bounded number of notches per sample, so a
    // long move returns promptly.
    let mut probe = Probe::new(Axis::Zoom, &[Notch { spacing: 1e-12, offset: 0.0, kind: 1 }], 0.5);
    probe.to(20.5, 0.0);
    let mut probe = Probe::new(Axis::Bearing, &[Notch { spacing: 1e-12, offset: 0.0, kind: 1 }], 0.5);
    probe.to(170.0, 0.0);
}

#[test]
fn a_disabled_axis_or_one_without_notches_reports_nothing() {
    let mut probe = Probe::new(Axis::Bearing, &NORTH, 355.0);
    probe.options.bearing.enabled = false;
    assert_eq!(probe.to(5.0, 0.0), None);
    let mut probe = Probe::new(Axis::Zoom, &[], 10.5);
    assert_eq!(probe.to(11.5, 0.0), None);
}
