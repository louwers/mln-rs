use mln_runtime::Completion;

use super::*;
use crate::diagnostic::tests::{diagnostic, message};
use crate::generated::types::{
    MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_OK, mln_gesture_delta, mln_gesture_input_data, mln_gesture_pointer,
    mln_gesture_pointer_down, mln_screen_point,
};
use crate::map::tests::{create_map, options as map_options, release_map};
use crate::runtime::tests::{create, release};

const NO_DIAGNOSTIC: *mut mln_diagnostic = core::ptr::null_mut();

fn point(x: f64, y: f64) -> mln_screen_point {
    mln_screen_point { x, y }
}

fn zeroed<T>() -> T {
    // SAFETY: the gesture structs hold numbers, bools and nullable pointers
    // only, for which zeros are valid.
    unsafe { core::mem::zeroed() }
}

fn input(type_: u32, data: mln_gesture_input_data) -> mln_gesture_input {
    mln_gesture_input { size: size_of::<mln_gesture_input>() as u32, type_, data }
}

fn down(id: u32, x: f64, y: f64, modes: u32) -> mln_gesture_input {
    let down = mln_gesture_pointer_down { id, kind: MLN_POINTER_KIND_MOUSE, position: point(x, y), modes };
    input(MLN_GESTURE_INPUT_POINTER_DOWN, mln_gesture_input_data { pointer_down: down })
}

fn pointer(type_: u32, id: u32, x: f64, y: f64) -> mln_gesture_input {
    let pointer = mln_gesture_pointer { id, position: point(x, y) };
    let data = match type_ {
        MLN_GESTURE_INPUT_POINTER_MOVE => mln_gesture_input_data { pointer_move: pointer },
        MLN_GESTURE_INPUT_POINTER_UP => mln_gesture_input_data { pointer_up: pointer },
        _ => mln_gesture_input_data { pointer_cancel: pointer },
    };
    input(type_, data)
}

fn empty_result() -> mln_gesture_result {
    mln_gesture_result { size: size_of::<mln_gesture_result>() as u32, ..zeroed() }
}

/// Handles `input` at `time`; the call's status and result.
fn handle(
    recognizer: mln_gesture_recognizer,
    input: &mln_gesture_input,
    time: f64,
) -> (mln_status, mln_gesture_result) {
    let mut result = empty_result();
    // SAFETY: local arguments.
    let status = unsafe { mln_gesture_recognizer_handle(recognizer, input, time, &raw mut result, NO_DIAGNOSTIC) };
    (status, result)
}

fn events(result: &mln_gesture_result) -> &[mln_gesture_event] {
    &result.events[..result.event_count]
}

fn create_recognizer(map: mln_map, options: Option<&mln_gesture_options>) -> Result<mln_gesture_recognizer, String> {
    let mut recognizer = 0;
    let mut out = diagnostic();
    let options = options.map_or(core::ptr::null(), core::ptr::from_ref);
    // SAFETY: local arguments.
    let status = unsafe { mln_gesture_recognizer_create(map, options, &raw mut recognizer, &raw mut *out) };
    if status == MLN_STATUS_OK { Ok(recognizer) } else { Err(message(&out).to_owned()) }
}

fn options() -> mln_gesture_options {
    mln_gesture_options { size: size_of::<mln_gesture_options>() as u32, ..zeroed() }
}

/// The options that `recognizer` holds.
fn held(recognizer: mln_gesture_recognizer) -> GestureOptions {
    RECOGNIZERS.get(recognizer).unwrap().lock().gestures.options().clone()
}

#[test]
fn a_drag_moves_the_map_and_taps_come_back_as_events() {
    let runtime = create();
    let map = create_map(runtime, &map_options(1.0));
    let recognizer = create_recognizer(map, None).unwrap();

    // A drag to the right moves the content right: the center goes west.
    assert_eq!(handle(recognizer, &down(1, 10.0, 16.0, 0), 0.0).0, MLN_STATUS_OK);
    for step in 1..=5 {
        let x = 10.0 + 6.0 * f64::from(step);
        let (status, result) =
            handle(recognizer, &pointer(MLN_GESTURE_INPUT_POINTER_MOVE, 1, x, 16.0), 16.0 * f64::from(step));
        assert_eq!(status, MLN_STATUS_OK);
        assert!(result.handled);
    }
    // Held still past the release window, so no glide follows.
    handle(recognizer, &pointer(MLN_GESTURE_INPUT_POINTER_UP, 1, 40.0, 16.0), 1000.0);
    let camera = map::resolve(map).unwrap().map.camera().and_then(Completion::wait).unwrap().camera;
    assert!(camera.center.longitude() < 0.0, "{:?}", camera.center);

    // Two taps: a tap, then a tap and a double tap.
    handle(recognizer, &down(2, 30.0, 16.0, 0), 2000.0);
    let (_, result) = handle(recognizer, &pointer(MLN_GESTURE_INPUT_POINTER_UP, 2, 30.0, 16.0), 2050.0);
    assert_eq!(events(&result).iter().map(|event| event.type_).collect::<Vec<_>>(), [MLN_GESTURE_EVENT_TAP]);
    handle(recognizer, &down(2, 30.0, 16.0, 0), 2100.0);
    let (_, result) = handle(recognizer, &pointer(MLN_GESTURE_INPUT_POINTER_UP, 2, 30.0, 16.0), 2150.0);
    let types: Vec<_> = events(&result).iter().map(|event| event.type_).collect();
    assert_eq!(types, [MLN_GESTURE_EVENT_TAP, MLN_GESTURE_EVENT_DOUBLE_TAP]);
    // SAFETY: the event is a double tap.
    let tap = unsafe { events(&result)[1].data.double_tap };
    assert_eq!((tap.id, tap.position.x, tap.position.y), (2, 30.0, 16.0));

    // A box-zoom drag reports its box while it is drawn, and a cancel drops it.
    handle(recognizer, &down(3, 10.0, 10.0, MLN_GESTURE_MODE_BOX_ZOOM), 3000.0);
    let (_, result) = handle(recognizer, &pointer(MLN_GESTURE_INPUT_POINTER_MOVE, 3, 30.0, 20.0), 3016.0);
    assert!(result.box_zoom_active);
    assert_eq!((result.box_zoom.min.x, result.box_zoom.max.x), (10.0, 30.0));
    let (_, result) = handle(recognizer, &pointer(MLN_GESTURE_INPUT_POINTER_CANCEL, 3, 0.0, 0.0), 3032.0);
    assert!(!result.box_zoom_active);

    mln_gesture_recognizer_release(recognizer);
    release_map(map);
    release(runtime);
}

#[test]
fn a_discrete_delta_asks_for_ticks_until_it_idles() {
    let runtime = create();
    let map = create_map(runtime, &map_options(1.0));
    let recognizer = create_recognizer(map, None).unwrap();
    let delta = mln_gesture_delta { position: point(32.0, 16.0), discrete: true, zoom: 1.0, ..zeroed() };
    let (status, result) = handle(recognizer, &input(MLN_GESTURE_INPUT_DELTA, mln_gesture_input_data { delta }), 0.0);
    assert_eq!(status, MLN_STATUS_OK);
    assert!(result.handled);
    assert_eq!(result.next_tick, MLN_GESTURE_NEXT_TICK_FRAME);
    let mut time = 0.0;
    loop {
        time += 16.0;
        let mut result = empty_result();
        // SAFETY: local arguments.
        assert_eq!(
            unsafe { mln_gesture_recognizer_tick(recognizer, time, &raw mut result, NO_DIAGNOSTIC) },
            MLN_STATUS_OK
        );
        if result.next_tick == MLN_GESTURE_NEXT_TICK_NONE {
            break;
        }
        assert!(time < 2000.0, "the delta idles");
    }
    let camera = map::resolve(map).unwrap().map.camera().and_then(Completion::wait).unwrap().camera;
    assert!((camera.zoom - 1.0).abs() < 1e-6, "{}", camera.zoom);
    mln_gesture_recognizer_release(recognizer);
    release_map(map);
    release(runtime);
}

#[test]
fn options_change_only_what_their_bits_set() {
    let runtime = create();
    let map = create_map(runtime, &map_options(1.0));
    let recognizer = create_recognizer(map, None).unwrap();
    assert_eq!(held(recognizer), GestureOptions::default());

    // A new model starts from its defaults: a curve from Android's fling.
    let mut next = options();
    next.pan.fields = MLN_GESTURE_PAN_OPTION_MOMENTUM;
    next.pan.momentum.fields = MLN_MOMENTUM_DECAY_MODEL | MLN_MOMENTUM_DECAY_MIN_SPEED;
    next.pan.momentum.model = MLN_MOMENTUM_DECAY_MODEL_CURVE;
    next.pan.momentum.min_speed = 40.0;
    next.zoom.fields = MLN_GESTURE_AXIS_OPTION_SNAPS;
    next.zoom.snaps = true;
    let notches = [mln_gesture_notch { spacing: 90.0, offset: 0.0, kind: 3 }];
    next.bearing.fields = MLN_GESTURE_AXIS_OPTION_NOTCHES | MLN_GESTURE_AXIS_OPTION_GLIDES;
    next.bearing.notches = notches.as_ptr();
    next.bearing.notch_count = notches.len();
    next.bearing.glides = false;
    // SAFETY: local arguments.
    assert_eq!(
        unsafe { mln_gesture_recognizer_set_options(recognizer, &raw const next, NO_DIAGNOSTIC) },
        MLN_STATUS_OK
    );
    let held = held(recognizer);
    assert_eq!(held.pan.momentum, Some(MomentumDecay { min_speed: 40.0, ..MomentumDecay::ANDROID_FLING }));
    assert_eq!(held.zoom.snap, Some(MomentumSnap::ZOOM));
    assert_eq!(held.bearing.notches, [Notch { spacing: 90.0, offset: 0.0, kind: 3 }]);
    assert_eq!(held.bearing.momentum, None);
    assert_eq!(held.bearing.snap, Some(MomentumSnap::BEARING));
    assert_eq!(held.pitch, GestureOptions::default().pitch);

    // Settings of the other model, and settings out of range, fail and
    // change nothing.
    let mut bad = options();
    bad.pan.fields = MLN_GESTURE_PAN_OPTION_MOMENTUM;
    bad.pan.momentum.fields = MLN_MOMENTUM_DECAY_HALF_LIFE_MS;
    bad.pan.momentum.half_life_ms = 100.0;
    let mut out = diagnostic();
    // SAFETY: local arguments.
    assert_eq!(
        unsafe { mln_gesture_recognizer_set_options(recognizer, &raw const bad, &raw mut *out) },
        MLN_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(message(&out), "the momentum of pan sets a setting of friction on a curve");
    let mut bad = options();
    let zero = [mln_gesture_notch { spacing: 0.0, offset: 0.0, kind: 0 }];
    bad.zoom.fields = MLN_GESTURE_AXIS_OPTION_NOTCHES;
    bad.zoom.notches = zero.as_ptr();
    bad.zoom.notch_count = 1;
    assert!(create_recognizer(map, Some(&bad)).unwrap_err().contains("a notch of zoom"));
    // SAFETY: local arguments.
    assert_eq!(
        unsafe { mln_gesture_recognizer_set_options(recognizer, &raw const bad, NO_DIAGNOSTIC) },
        MLN_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(self::held(recognizer), held);

    mln_gesture_recognizer_release(recognizer);
    release_map(map);
    release(runtime);
}

#[test]
fn invalid_inputs_fail() {
    let runtime = create();
    let map = create_map(runtime, &map_options(1.0));
    let recognizer = create_recognizer(map, None).unwrap();
    assert_eq!(handle(recognizer, &input(9, zeroed()), 0.0).0, MLN_STATUS_INVALID_ARGUMENT);
    assert_eq!(
        handle(recognizer, &input(MLN_GESTURE_INPUT_MODES, mln_gesture_input_data { modes: 8 }), 0.0).0,
        MLN_STATUS_INVALID_ARGUMENT
    );
    let delta = mln_gesture_delta { zoom: f64::NAN, ..zeroed() };
    assert_eq!(
        handle(recognizer, &input(MLN_GESTURE_INPUT_DELTA, mln_gesture_input_data { delta }), 0.0).0,
        MLN_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(handle(recognizer, &down(1, f64::INFINITY, 0.0, 0), 0.0).0, MLN_STATUS_INVALID_ARGUMENT);
    mln_gesture_recognizer_release(recognizer);
    release_map(map);
    release(runtime);
}

/// Runs frames from `from` until the recognizer needs none; the events they
/// reported and the time it stopped at.
fn frames(recognizer: mln_gesture_recognizer, from: f64) -> (Vec<u32>, f64) {
    let mut types = Vec::new();
    let mut time = from;
    loop {
        time += 16.0;
        let mut result = empty_result();
        // SAFETY: local arguments.
        assert_eq!(
            unsafe { mln_gesture_recognizer_tick(recognizer, time, &raw mut result, NO_DIAGNOSTIC) },
            MLN_STATUS_OK
        );
        types.extend(events(&result).iter().map(|event| event.type_));
        if result.next_tick == MLN_GESTURE_NEXT_TICK_NONE {
            return (types, time);
        }
        assert!(time - from < 2000.0, "the recognizer settles");
    }
}

fn zoom(map: mln_map) -> f64 {
    map::resolve(map).unwrap().map.camera().and_then(Completion::wait).unwrap().camera.zoom
}

#[test]
fn taps_zoom_as_the_options_and_modes_say_and_a_held_press_is_a_long_press() {
    let runtime = create();
    let map = create_map(runtime, &map_options(1.0));
    let mut options = options();
    options.fields = MLN_GESTURE_OPTION_DOUBLE_TAP_ZOOM;
    options.double_tap_zoom = 2.0;
    let recognizer = create_recognizer(map, Some(&options)).unwrap();
    assert_eq!(held(recognizer).double_tap_zoom, 2.0);
    assert_eq!(held(recognizer).two_finger_tap_zoom, -1.0);

    // A double tap zooms in its two levels, and one with the zoom-out mode
    // back out.
    let start = zoom(map);
    let mut time = 0.0;
    for (modes, expected) in [(0, start + 2.0), (MLN_GESTURE_MODE_ZOOM_OUT, start)] {
        for _ in 0..2 {
            handle(recognizer, &down(1, 30.0, 16.0, modes), time);
            handle(recognizer, &pointer(MLN_GESTURE_INPUT_POINTER_UP, 1, 30.0, 16.0), time + 30.0);
            time += 100.0;
        }
        time = frames(recognizer, time).1 + 1000.0;
        assert!((zoom(map) - expected).abs() < 1e-6, "{} != {expected}", zoom(map));
    }

    // A press held still: frames time it, and its lift makes no tap.
    let time = time + 1000.0;
    let (status, result) = handle(recognizer, &down(2, 30.0, 16.0, 0), time);
    assert_eq!(status, MLN_STATUS_OK);
    assert_eq!((result.next_tick, result.next_tick_ms), (MLN_GESTURE_NEXT_TICK_AT, time + 500.0), "the press is timed");
    let mut long_pressed = false;
    let mut now = time;
    while !long_pressed {
        now += 16.0;
        let mut result = empty_result();
        // SAFETY: local arguments.
        assert_eq!(
            unsafe { mln_gesture_recognizer_tick(recognizer, now, &raw mut result, NO_DIAGNOSTIC) },
            MLN_STATUS_OK
        );
        long_pressed = events(&result).iter().any(|event| event.type_ == MLN_GESTURE_EVENT_LONG_PRESS);
        assert!(now - time < 600.0, "a long press by 500 ms");
    }
    let (_, result) = handle(recognizer, &pointer(MLN_GESTURE_INPUT_POINTER_UP, 2, 30.0, 16.0), now + 16.0);
    assert_eq!(result.event_count, 0);

    mln_gesture_recognizer_release(recognizer);
    release_map(map);
    release(runtime);
}
