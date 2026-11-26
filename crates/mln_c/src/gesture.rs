//! `gesture.h`: a gesture recognizer that moves a map's camera.
//!
//! Each recognizer is an `mln_engine::gestures::GestureRecognizer` with the runtime
//! map it moves, behind a lock: calls run on the caller's thread, and the
//! camera moves they make go to the map as commands.

use std::collections::VecDeque;
use std::sync::Arc;

use ::mln_engine::camera::{MomentumDecay, MomentumDecayModel, MomentumEasing, MomentumSnap};
use mln_engine::gestures::{
    AxisOptions, GestureEvent, GestureOptions, GestureRecognizer, Input, Modes, NextTick, Notch, PinchPhase,
    PointerKind,
};
use mln_util::geo::ScreenCoordinate;
use parking_lot::Mutex;

use crate::abi::{self, Fields};
use crate::camera::{duration, is_duration_ms};
use crate::error::Error;
use crate::generated::types::{
    MLN_GESTURE_AXIS_BEARING, MLN_GESTURE_AXIS_OPTION_ENABLED, MLN_GESTURE_AXIS_OPTION_GLIDES,
    MLN_GESTURE_AXIS_OPTION_MOMENTUM, MLN_GESTURE_AXIS_OPTION_NOTCHES, MLN_GESTURE_AXIS_OPTION_SNAP,
    MLN_GESTURE_AXIS_OPTION_SNAPS, MLN_GESTURE_AXIS_PITCH, MLN_GESTURE_AXIS_ZOOM, MLN_GESTURE_EVENT_DOUBLE_TAP,
    MLN_GESTURE_EVENT_LONG_PRESS, MLN_GESTURE_EVENT_NOTCH, MLN_GESTURE_EVENT_TAP, MLN_GESTURE_EVENT_TWO_FINGER_TAP,
    MLN_GESTURE_INPUT_DELTA, MLN_GESTURE_INPUT_MODES, MLN_GESTURE_INPUT_PINCH, MLN_GESTURE_INPUT_POINTER_CANCEL,
    MLN_GESTURE_INPUT_POINTER_DOWN, MLN_GESTURE_INPUT_POINTER_MOVE, MLN_GESTURE_INPUT_POINTER_UP,
    MLN_GESTURE_MODE_BOX_ZOOM, MLN_GESTURE_MODE_ROTATE, MLN_GESTURE_MODE_ZOOM_OUT, MLN_GESTURE_NEXT_TICK_AT,
    MLN_GESTURE_NEXT_TICK_FRAME, MLN_GESTURE_NEXT_TICK_NONE, MLN_GESTURE_OPTION_DOUBLE_TAP_ZOOM,
    MLN_GESTURE_OPTION_TWO_FINGER_TAP_ZOOM, MLN_GESTURE_PAN_OPTION_ENABLED, MLN_GESTURE_PAN_OPTION_GLIDES,
    MLN_GESTURE_PAN_OPTION_MOMENTUM, MLN_GESTURE_RESULT_EVENT_CAPACITY, MLN_MOMENTUM_DECAY_DURATION_EXPONENT,
    MLN_MOMENTUM_DECAY_DURATION_MS, MLN_MOMENTUM_DECAY_EASING, MLN_MOMENTUM_DECAY_HALF_LIFE_MS,
    MLN_MOMENTUM_DECAY_MAX_DISTANCE, MLN_MOMENTUM_DECAY_MAX_SPEED, MLN_MOMENTUM_DECAY_MIN_SPEED,
    MLN_MOMENTUM_DECAY_MODEL, MLN_MOMENTUM_DECAY_MODEL_CURVE, MLN_MOMENTUM_DECAY_MODEL_FRICTION,
    MLN_MOMENTUM_DECAY_REFERENCE_SPEED, MLN_MOMENTUM_DECAY_STOP_SPEED, MLN_MOMENTUM_SNAP_DURATION_MS,
    MLN_MOMENTUM_SNAP_OFFSET, MLN_MOMENTUM_SNAP_SPACING, MLN_MOMENTUM_SNAP_TOLERANCE, MLN_PINCH_PHASE_BEGIN,
    MLN_PINCH_PHASE_CANCEL, MLN_PINCH_PHASE_END, MLN_PINCH_PHASE_UPDATE, MLN_POINTER_KIND_MOUSE, MLN_POINTER_KIND_PEN,
    MLN_POINTER_KIND_TOUCH, mln_diagnostic, mln_gesture_axis_options, mln_gesture_event, mln_gesture_event_data,
    mln_gesture_input, mln_gesture_notch, mln_gesture_notch_event, mln_gesture_options, mln_gesture_pan_options,
    mln_gesture_recognizer, mln_gesture_result, mln_gesture_tap, mln_map, mln_momentum_decay, mln_momentum_snap,
    mln_screen_box, mln_status,
};
use crate::handles::{self, HandleKind, HandleTable};
use crate::{geo, map};

abi::versioned!(mln_gesture_options, mln_gesture_input, mln_gesture_result);

/// A recognizer, the map it moves, and the events that a call couldn't fit
/// in its result, which the next one reports first.
struct Recognizer {
    gestures: GestureRecognizer<::mln_runtime::Map>,
    events: VecDeque<GestureEvent>,
}

static RECOGNIZERS: HandleTable<Mutex<Recognizer>> = HandleTable::new(HandleKind::GestureRecognizer);

// MARK: - Options

/// `options` over `current`, checked.
fn options(options: &mln_gesture_options, current: &GestureOptions) -> Result<GestureOptions, Error> {
    let mut next = current.clone();
    let fields = Fields::new(
        options.fields,
        MLN_GESTURE_OPTION_DOUBLE_TAP_ZOOM | MLN_GESTURE_OPTION_TWO_FINGER_TAP_ZOOM,
        "mln_gesture_options.fields",
    )?;
    if let Some(levels) = fields.get(MLN_GESTURE_OPTION_DOUBLE_TAP_ZOOM, options.double_tap_zoom) {
        next.double_tap_zoom = levels;
    }
    if let Some(levels) = fields.get(MLN_GESTURE_OPTION_TWO_FINGER_TAP_ZOOM, options.two_finger_tap_zoom) {
        next.two_finger_tap_zoom = levels;
    }
    pan(&options.pan, &mut next)?;
    let parts = [
        (&options.zoom, &mut next.zoom, MomentumDecay::ZOOM, MomentumSnap::ZOOM, "zoom"),
        (&options.bearing, &mut next.bearing, MomentumDecay::BEARING, MomentumSnap::BEARING, "bearing"),
        (&options.pitch, &mut next.pitch, MomentumDecay::PITCH, MomentumSnap::PITCH, "pitch"),
    ];
    for (options, part, decay, snap, name) in parts {
        // SAFETY: the caller's contract, which covers each part's notches.
        unsafe { axis(options, part, decay, snap, name) }?;
    }
    next.validate().map_err(|error| Error::invalid_argument(error.to_string()))?;
    Ok(next)
}

fn pan(options: &mln_gesture_pan_options, next: &mut GestureOptions) -> Result<(), Error> {
    let fields = Fields::new(
        options.fields,
        MLN_GESTURE_PAN_OPTION_ENABLED | MLN_GESTURE_PAN_OPTION_GLIDES | MLN_GESTURE_PAN_OPTION_MOMENTUM,
        "mln_gesture_pan_options.fields",
    )?;
    if let Some(enabled) = fields.get(MLN_GESTURE_PAN_OPTION_ENABLED, options.enabled) {
        next.pan.enabled = enabled;
    }
    next.pan.momentum = glide(
        next.pan.momentum,
        fields.get(MLN_GESTURE_PAN_OPTION_GLIDES, options.glides),
        fields.get(MLN_GESTURE_PAN_OPTION_MOMENTUM, &options.momentum),
        MomentumDecay::PAN,
        "pan",
    )?;
    Ok(())
}

/// # Safety
///
/// `options.notches` is null, or points to `notch_count` notches.
unsafe fn axis(
    options: &mln_gesture_axis_options,
    part: &mut AxisOptions,
    default_decay: MomentumDecay,
    default_snap: MomentumSnap,
    name: &str,
) -> Result<(), Error> {
    let fields = Fields::new(
        options.fields,
        MLN_GESTURE_AXIS_OPTION_ENABLED
            | MLN_GESTURE_AXIS_OPTION_GLIDES
            | MLN_GESTURE_AXIS_OPTION_MOMENTUM
            | MLN_GESTURE_AXIS_OPTION_SNAPS
            | MLN_GESTURE_AXIS_OPTION_SNAP
            | MLN_GESTURE_AXIS_OPTION_NOTCHES,
        "mln_gesture_axis_options.fields",
    )?;
    if let Some(enabled) = fields.get(MLN_GESTURE_AXIS_OPTION_ENABLED, options.enabled) {
        part.enabled = enabled;
    }
    part.momentum = glide(
        part.momentum,
        fields.get(MLN_GESTURE_AXIS_OPTION_GLIDES, options.glides),
        fields.get(MLN_GESTURE_AXIS_OPTION_MOMENTUM, &options.momentum),
        default_decay,
        name,
    )?;
    let snap = fields
        .get(MLN_GESTURE_AXIS_OPTION_SNAP, &options.snap)
        .map(|options| snap(options, part.snap.unwrap_or(default_snap)));
    part.snap = match (fields.get(MLN_GESTURE_AXIS_OPTION_SNAPS, options.snaps), snap) {
        (Some(false), _) => None,
        (_, Some(snap)) => Some(snap?),
        (Some(true), None) => Some(part.snap.unwrap_or(default_snap)),
        (None, None) => part.snap,
    };
    if fields.get(MLN_GESTURE_AXIS_OPTION_NOTCHES, ()).is_some() {
        if options.notches.is_null() && options.notch_count != 0 {
            return Err(Error::invalid_argument(format!("the notches of {name} must not be null")));
        }
        let notches: &[mln_gesture_notch] = if options.notch_count == 0 {
            &[]
        } else {
            // SAFETY: non-null, and the caller's contract.
            unsafe { core::slice::from_raw_parts(options.notches, options.notch_count) }
        };
        part.notches = notches
            .iter()
            .map(|notch| Notch { spacing: notch.spacing, offset: notch.offset, kind: notch.kind })
            .collect();
    }
    Ok(())
}

/// A part's momentum: off when `glides` turns it off; otherwise `settings`
/// over the current decay, or the part's default when it had none.
fn glide(
    current: Option<MomentumDecay>,
    glides: Option<bool>,
    settings: Option<&mln_momentum_decay>,
    default: MomentumDecay,
    part: &str,
) -> Result<Option<MomentumDecay>, Error> {
    if glides == Some(false) {
        return Ok(None);
    }
    let base = current.unwrap_or(default);
    match settings {
        Some(settings) => decay(settings, base, default, part).map(Some),
        None if glides == Some(true) => Ok(Some(base)),
        None => Ok(current),
    }
}

/// `options` over `base`. A new model starts from its defaults: friction
/// from the part's, `default`, and a curve from Android's fling.
fn decay(
    options: &mln_momentum_decay,
    base: MomentumDecay,
    default: MomentumDecay,
    part: &str,
) -> Result<MomentumDecay, Error> {
    let fail = |problem: &str| Error::invalid_argument(format!("the momentum of {part} {problem}"));
    let friction_bits = MLN_MOMENTUM_DECAY_HALF_LIFE_MS | MLN_MOMENTUM_DECAY_STOP_SPEED;
    let curve_bits = MLN_MOMENTUM_DECAY_REFERENCE_SPEED
        | MLN_MOMENTUM_DECAY_DURATION_MS
        | MLN_MOMENTUM_DECAY_DURATION_EXPONENT
        | MLN_MOMENTUM_DECAY_EASING;
    let fields = Fields::new(
        options.fields,
        MLN_MOMENTUM_DECAY_MODEL
            | friction_bits
            | curve_bits
            | MLN_MOMENTUM_DECAY_MIN_SPEED
            | MLN_MOMENTUM_DECAY_MAX_SPEED
            | MLN_MOMENTUM_DECAY_MAX_DISTANCE,
        "mln_momentum_decay.fields",
    )?;
    let mut decay = match fields.get(MLN_MOMENTUM_DECAY_MODEL, options.model) {
        None => base,
        Some(MLN_MOMENTUM_DECAY_MODEL_FRICTION) if matches!(base.model, MomentumDecayModel::Friction { .. }) => base,
        Some(MLN_MOMENTUM_DECAY_MODEL_FRICTION) => default,
        Some(MLN_MOMENTUM_DECAY_MODEL_CURVE) if matches!(base.model, MomentumDecayModel::Curve { .. }) => base,
        Some(MLN_MOMENTUM_DECAY_MODEL_CURVE) => MomentumDecay::ANDROID_FLING,
        Some(model) => return Err(fail(&format!("has an invalid model {model}"))),
    };
    let positive_ms = |ms: f64| (ms.is_finite() && ms > 0.0 && is_duration_ms(ms)).then(|| duration(ms));
    match &mut decay.model {
        MomentumDecayModel::Friction { half_life, stop_speed } => {
            if options.fields & curve_bits != 0 {
                return Err(fail("sets a setting of a curve on friction"));
            }
            if let Some(ms) = fields.get(MLN_MOMENTUM_DECAY_HALF_LIFE_MS, options.half_life_ms) {
                *half_life = positive_ms(ms).ok_or_else(|| fail("needs a positive half-life in the native range"))?;
            }
            *stop_speed = fields.get(MLN_MOMENTUM_DECAY_STOP_SPEED, options.stop_speed).unwrap_or(*stop_speed);
        }
        MomentumDecayModel::Curve { reference_speed, duration: curve_duration, exponent, easing } => {
            if options.fields & friction_bits != 0 {
                return Err(fail("sets a setting of friction on a curve"));
            }
            *reference_speed =
                fields.get(MLN_MOMENTUM_DECAY_REFERENCE_SPEED, options.reference_speed).unwrap_or(*reference_speed);
            if let Some(ms) = fields.get(MLN_MOMENTUM_DECAY_DURATION_MS, options.duration_ms) {
                *curve_duration =
                    positive_ms(ms).ok_or_else(|| fail("needs a positive duration in the native range"))?;
            }
            *exponent =
                fields.get(MLN_MOMENTUM_DECAY_DURATION_EXPONENT, options.duration_exponent).unwrap_or(*exponent);
            if let Some(curve) = fields.get(MLN_MOMENTUM_DECAY_EASING, options.easing) {
                *easing = MomentumEasing { x1: curve.x1, y1: curve.y1, x2: curve.x2, y2: curve.y2 };
            }
        }
    }
    decay.min_speed = fields.get(MLN_MOMENTUM_DECAY_MIN_SPEED, options.min_speed).unwrap_or(decay.min_speed);
    decay.max_speed = fields.get(MLN_MOMENTUM_DECAY_MAX_SPEED, options.max_speed).unwrap_or(decay.max_speed);
    decay.max_distance =
        fields.get(MLN_MOMENTUM_DECAY_MAX_DISTANCE, options.max_distance).unwrap_or(decay.max_distance);
    decay.validate().map_err(|error| fail(&error.to_string()))?;
    Ok(decay)
}

/// `options` over `snap`.
fn snap(options: &mln_momentum_snap, mut snap: MomentumSnap) -> Result<MomentumSnap, Error> {
    let fields = Fields::new(
        options.fields,
        MLN_MOMENTUM_SNAP_SPACING
            | MLN_MOMENTUM_SNAP_OFFSET
            | MLN_MOMENTUM_SNAP_TOLERANCE
            | MLN_MOMENTUM_SNAP_DURATION_MS,
        "mln_momentum_snap.fields",
    )?;
    snap.spacing = fields.get(MLN_MOMENTUM_SNAP_SPACING, options.spacing).unwrap_or(snap.spacing);
    snap.offset = fields.get(MLN_MOMENTUM_SNAP_OFFSET, options.offset).unwrap_or(snap.offset);
    snap.tolerance = fields.get(MLN_MOMENTUM_SNAP_TOLERANCE, options.tolerance).unwrap_or(snap.tolerance);
    if let Some(ms) = fields.get(MLN_MOMENTUM_SNAP_DURATION_MS, options.duration_ms) {
        if !is_duration_ms(ms) {
            return Err(Error::invalid_argument("a momentum snap needs a duration in the native range"));
        }
        snap.duration = duration(ms);
    }
    snap.validate().map_err(|error| Error::invalid_argument(error.to_string()))?;
    Ok(snap)
}

// MARK: - Inputs and results

fn modes(bits: u32) -> Result<Modes, Error> {
    let fields = Fields::new(
        bits,
        MLN_GESTURE_MODE_ROTATE | MLN_GESTURE_MODE_BOX_ZOOM | MLN_GESTURE_MODE_ZOOM_OUT,
        "mln_gesture_modes",
    )?;
    Ok(Modes {
        rotate: fields.get(MLN_GESTURE_MODE_ROTATE, ()).is_some(),
        box_zoom: fields.get(MLN_GESTURE_MODE_BOX_ZOOM, ()).is_some(),
        zoom_out: fields.get(MLN_GESTURE_MODE_ZOOM_OUT, ()).is_some(),
    })
}

fn finite(value: f64, name: &str) -> Result<f64, Error> {
    if value.is_finite() { Ok(value) } else { Err(Error::invalid_argument(format!("{name} must be finite"))) }
}

fn input(input: &mln_gesture_input) -> Result<Input, Error> {
    // SAFETY: each arm reads the member that `type_` names.
    Ok(unsafe {
        match input.type_ {
            MLN_GESTURE_INPUT_POINTER_DOWN => {
                let down = input.data.pointer_down;
                let kind = match down.kind {
                    MLN_POINTER_KIND_MOUSE => PointerKind::Mouse,
                    MLN_POINTER_KIND_TOUCH => PointerKind::Touch,
                    MLN_POINTER_KIND_PEN => PointerKind::Pen,
                    kind => return Err(Error::invalid_argument(format!("pointer kind {kind} is invalid"))),
                };
                Input::PointerDown {
                    id: down.id,
                    kind,
                    position: geo::screen_point(down.position)?,
                    modes: modes(down.modes)?,
                }
            }
            MLN_GESTURE_INPUT_POINTER_MOVE => {
                let pointer = input.data.pointer_move;
                Input::PointerMove { id: pointer.id, position: geo::screen_point(pointer.position)? }
            }
            MLN_GESTURE_INPUT_POINTER_UP => {
                let pointer = input.data.pointer_up;
                Input::PointerUp { id: pointer.id, position: geo::screen_point(pointer.position)? }
            }
            MLN_GESTURE_INPUT_POINTER_CANCEL => Input::PointerCancel { id: input.data.pointer_cancel.id },
            MLN_GESTURE_INPUT_MODES => Input::Modes(modes(input.data.modes)?),
            MLN_GESTURE_INPUT_DELTA => {
                let delta = input.data.delta;
                Input::Delta {
                    position: geo::screen_point(delta.position)?,
                    discrete: delta.discrete,
                    pan: geo::screen_point(delta.pan)?,
                    zoom: finite(delta.zoom, "a delta's zoom")?,
                    bearing: finite(delta.bearing, "a delta's bearing")?,
                    pitch: finite(delta.pitch, "a delta's pitch")?,
                }
            }
            MLN_GESTURE_INPUT_PINCH => {
                let pinch = input.data.pinch;
                let phase = match pinch.phase {
                    MLN_PINCH_PHASE_BEGIN => PinchPhase::Begin,
                    MLN_PINCH_PHASE_UPDATE => PinchPhase::Update,
                    MLN_PINCH_PHASE_END => PinchPhase::End,
                    MLN_PINCH_PHASE_CANCEL => PinchPhase::Cancel,
                    phase => return Err(Error::invalid_argument(format!("pinch phase {phase} is invalid"))),
                };
                if !(pinch.scale.is_finite() && pinch.scale > 0.0) {
                    return Err(Error::invalid_argument("a pinch's scale must be positive and finite"));
                }
                Input::Pinch {
                    center: geo::screen_point(pinch.center)?,
                    scale: pinch.scale,
                    rotation: finite(pinch.rotation, "a pinch's rotation")?,
                    phase,
                }
            }
            kind => return Err(Error::invalid_argument(format!("gesture input type {kind} is invalid"))),
        }
    })
}

fn event(event: GestureEvent) -> mln_gesture_event {
    let tap = |id: u32, position: ScreenCoordinate| mln_gesture_tap { id, position: geo::from_screen_point(position) };
    let (type_, data) = match event {
        GestureEvent::Tap { id, position } => {
            (MLN_GESTURE_EVENT_TAP, mln_gesture_event_data { tap: tap(id, position) })
        }
        GestureEvent::LongPress { id, position } => {
            (MLN_GESTURE_EVENT_LONG_PRESS, mln_gesture_event_data { long_press: tap(id, position) })
        }
        GestureEvent::DoubleTap { id, position } => {
            (MLN_GESTURE_EVENT_DOUBLE_TAP, mln_gesture_event_data { double_tap: tap(id, position) })
        }
        GestureEvent::TwoFingerTap { position } => {
            (MLN_GESTURE_EVENT_TWO_FINGER_TAP, mln_gesture_event_data { two_finger_tap: tap(0, position) })
        }
        GestureEvent::Notch { axis, value, kind } => {
            let axis = match axis {
                mln_engine::gestures::Axis::Zoom => MLN_GESTURE_AXIS_ZOOM,
                mln_engine::gestures::Axis::Bearing => MLN_GESTURE_AXIS_BEARING,
                mln_engine::gestures::Axis::Pitch => MLN_GESTURE_AXIS_PITCH,
            };
            (MLN_GESTURE_EVENT_NOTCH, mln_gesture_event_data { notch: mln_gesture_notch_event { axis, kind, value } })
        }
    };
    mln_gesture_event { type_, data }
}

impl Recognizer {
    /// What the call just made did, with as many of the events as fit.
    fn result(&mut self, handled: bool, next_tick: Option<NextTick>) -> mln_gesture_result {
        self.events.extend(self.gestures.take_events());
        // SAFETY: zeros are a valid result: no events, no box.
        let mut result: mln_gesture_result = unsafe { core::mem::zeroed() };
        result.handled = handled;
        (result.next_tick, result.next_tick_ms) = match next_tick {
            None => (MLN_GESTURE_NEXT_TICK_NONE, 0.0),
            Some(NextTick::Frame) => (MLN_GESTURE_NEXT_TICK_FRAME, 0.0),
            Some(NextTick::At(ms)) => (MLN_GESTURE_NEXT_TICK_AT, ms),
        };
        if let Some([a, b]) = self.gestures.box_zoom() {
            result.box_zoom_active = true;
            result.box_zoom = mln_screen_box { min: geo::from_screen_point(a), max: geo::from_screen_point(b) };
        }
        let count = self.events.len().min(MLN_GESTURE_RESULT_EVENT_CAPACITY as usize);
        for (slot, taken) in result.events.iter_mut().zip(self.events.drain(..count)) {
            *slot = event(taken);
        }
        result.event_count = count;
        result
    }
}

// MARK: - Functions

/// `mln_gesture_recognizer_create()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_gesture_recognizer_create(
    map: mln_map,
    options: *const mln_gesture_options,
    out_recognizer: *mut mln_gesture_recognizer,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        let out_recognizer = unsafe { handles::out_handle(out_recognizer, "out_recognizer") }?;
        let map = map::resolve(map)?.map.clone();
        let gesture_options = if options.is_null() {
            GestureOptions::default()
        } else {
            // SAFETY: the caller's contract.
            self::options(&unsafe { abi::read_sized(options) }?, &GestureOptions::default())?
        };
        let gestures = GestureRecognizer::with_options(map, gesture_options)
            .map_err(|error| Error::invalid_argument(error.to_string()))?;
        let recognizer = Recognizer { gestures, events: VecDeque::new() };
        *out_recognizer = RECOGNIZERS.insert(Arc::new(Mutex::new(recognizer)))?;
        Ok(())
    })
}

/// `mln_gesture_recognizer_release()`.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn mln_gesture_recognizer_release(recognizer: mln_gesture_recognizer) {
    let _ = release(recognizer);
}

/// Releases `recognizer`, as `mln_gesture_recognizer_release()` does.
pub(crate) fn release(recognizer: mln_gesture_recognizer) -> Result<(), Error> {
    RECOGNIZERS.remove(recognizer).map(drop)
}

/// `mln_gesture_recognizer_set_options()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_gesture_recognizer_set_options(
    recognizer: mln_gesture_recognizer,
    options: *const mln_gesture_options,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        let recognizer = RECOGNIZERS.get(recognizer)?;
        // SAFETY: the caller's contract.
        let options = unsafe { abi::read_sized(options) }?;
        let mut recognizer = recognizer.lock();
        let next = self::options(&options, recognizer.gestures.options())?;
        recognizer.gestures.set_options(next).map_err(|error| Error::invalid_argument(error.to_string()))
    })
}

/// `mln_gesture_recognizer_handle()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_gesture_recognizer_handle(
    recognizer: mln_gesture_recognizer,
    input: *const mln_gesture_input,
    time_ms: f64,
    out_result: *mut mln_gesture_result,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        let recognizer = RECOGNIZERS.get(recognizer)?;
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_result) }?;
        // SAFETY: the caller's contract.
        let input = self::input(&unsafe { abi::read_sized(input) }?)?;
        let time = finite(time_ms, "time_ms")?;
        let mut recognizer = recognizer.lock();
        let handled = recognizer.gestures.handle(input, time);
        let next_tick = recognizer.gestures.next_tick();
        let result = recognizer.result(handled, next_tick);
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_result, &result) }
    })
}

/// `mln_gesture_recognizer_tick()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_gesture_recognizer_tick(
    recognizer: mln_gesture_recognizer,
    time_ms: f64,
    out_result: *mut mln_gesture_result,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        let recognizer = RECOGNIZERS.get(recognizer)?;
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_result) }?;
        let time = finite(time_ms, "time_ms")?;
        let mut recognizer = recognizer.lock();
        let next_tick = recognizer.gestures.tick(time);
        let result = recognizer.result(false, next_tick);
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_result, &result) }
    })
}

#[cfg(test)]
#[path = "gesture_test.rs"]
mod gesture_test;
