//! A gesture's momentum, as `mln_camera_delta.momentum` gives it: each
//! friction or snap setting whose bit is set replaces the part's default.

use ::mln_map::{AnchoredMomentum, CameraMomentum, MomentumEasing, MomentumFriction, MomentumGlide, MomentumSnap};
use mln_util::geo::ScreenCoordinate;

use super::{duration, is_duration_ms};
use crate::abi::Fields;
use crate::error::Error;
use crate::generated::types::{
    MLN_ANCHORED_MOMENTUM_ANCHOR, MLN_ANCHORED_MOMENTUM_DELTA, MLN_ANCHORED_MOMENTUM_DURATION_MS,
    MLN_ANCHORED_MOMENTUM_EASING, MLN_ANCHORED_MOMENTUM_SNAP, MLN_ANCHORED_MOMENTUM_VELOCITY,
    MLN_MOMENTUM_FRICTION_HALF_LIFE_MS, MLN_MOMENTUM_FRICTION_MAX_DISTANCE, MLN_MOMENTUM_FRICTION_MAX_SPEED,
    MLN_MOMENTUM_FRICTION_MIN_SPEED, MLN_MOMENTUM_FRICTION_STOP_SPEED, MLN_MOMENTUM_SNAP_DURATION_MS,
    MLN_MOMENTUM_SNAP_OFFSET, MLN_MOMENTUM_SNAP_SPACING, MLN_MOMENTUM_SNAP_TOLERANCE, MLN_PAN_MOMENTUM_DELTA,
    MLN_PAN_MOMENTUM_DURATION_MS, MLN_PAN_MOMENTUM_EASING, MLN_PAN_MOMENTUM_VELOCITY, mln_anchored_momentum,
    mln_gesture_momentum, mln_momentum_friction, mln_momentum_snap, mln_pan_momentum, mln_unit_bezier,
};
use crate::geo;

/// A number that is finite and greater than 0.
fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

/// `options` over the part's `friction`. `part` names the part in
/// diagnostics.
fn friction(
    options: &mln_momentum_friction,
    defaults: MomentumFriction,
    part: &str,
) -> Result<MomentumFriction, Error> {
    let fields = Fields::new(
        options.fields,
        MLN_MOMENTUM_FRICTION_HALF_LIFE_MS
            | MLN_MOMENTUM_FRICTION_STOP_SPEED
            | MLN_MOMENTUM_FRICTION_MIN_SPEED
            | MLN_MOMENTUM_FRICTION_MAX_SPEED
            | MLN_MOMENTUM_FRICTION_MAX_DISTANCE,
        "mln_momentum_friction.fields",
    )?;
    let half_life_ms = fields.get(MLN_MOMENTUM_FRICTION_HALF_LIFE_MS, options.half_life_ms);
    let stop_speed = fields.get(MLN_MOMENTUM_FRICTION_STOP_SPEED, options.stop_speed);
    let min_speed = fields.get(MLN_MOMENTUM_FRICTION_MIN_SPEED, options.min_speed);
    let max_speed = fields.get(MLN_MOMENTUM_FRICTION_MAX_SPEED, options.max_speed);
    let max_distance = fields.get(MLN_MOMENTUM_FRICTION_MAX_DISTANCE, options.max_distance);
    if half_life_ms.is_some_and(|ms| !positive(ms) || !is_duration_ms(ms)) {
        return Err(Error::invalid_argument(format!(
            "the friction of {part} needs a positive half-life in the native duration range"
        )));
    }
    let valid = min_speed.is_none_or(|speed| speed.is_finite() && speed >= 0.0)
        && [stop_speed, max_speed, max_distance].into_iter().flatten().all(positive);
    if !valid {
        return Err(Error::invalid_argument(format!(
            "the friction of {part} needs a finite minimum speed that isn't negative, and finite positive values for \
             the rest"
        )));
    }
    Ok(MomentumFriction {
        half_life: half_life_ms.map_or(defaults.half_life, duration),
        stop_speed: stop_speed.unwrap_or(defaults.stop_speed),
        min_speed: min_speed.unwrap_or(defaults.min_speed),
        max_speed: max_speed.unwrap_or(defaults.max_speed),
        max_distance: max_distance.unwrap_or(defaults.max_distance),
    })
}

const NOT_FINITE_VELOCITY: &str = "gesture momentum velocities must be finite";
const NOT_FINITE_DELTA: &str = "gesture momentum deltas must be finite";

/// A part's glide settings, as its struct gives them, whatever the unit.
struct GlideFields<'a, T> {
    velocity: Option<T>,
    friction: &'a mln_momentum_friction,
    delta: Option<T>,
    duration_ms: Option<f64>,
    easing: Option<mln_unit_bezier>,
}

impl<T> GlideFields<'_, T> {
    /// The part's glide, if it has one: friction from `velocity`, over the
    /// part's `defaults`, or the planned `delta`. `part` names the part in
    /// diagnostics.
    fn glide(self, defaults: MomentumFriction, part: &str) -> Result<Option<MomentumGlide<T>>, Error> {
        let fail = |problem: &str| Error::invalid_argument(format!("the momentum of {part} {problem}"));
        match (self.velocity, self.delta) {
            (Some(_), Some(_)) => Err(fail("sets both a velocity and a delta")),
            (Some(velocity), None) => {
                if self.duration_ms.is_some() || self.easing.is_some() {
                    return Err(fail("sets a duration or an easing without a delta"));
                }
                Ok(Some(MomentumGlide::Friction { velocity, friction: friction(self.friction, defaults, part)? }))
            }
            (None, Some(delta)) => {
                let Some(duration_ms) = self.duration_ms.filter(|&ms| positive(ms) && is_duration_ms(ms)) else {
                    return Err(fail("needs a positive duration in the native range with its delta"));
                };
                let easing = match self.easing {
                    Some(curve) => {
                        if ![curve.x1, curve.y1, curve.x2, curve.y2].iter().all(|value| (0.0..=1.0).contains(value)) {
                            return Err(fail("needs an easing whose control points are within [0, 1]"));
                        }
                        MomentumEasing { x1: curve.x1, y1: curve.y1, x2: curve.x2, y2: curve.y2 }
                    }
                    None => MomentumEasing::DEFAULT,
                };
                Ok(Some(MomentumGlide::Planned { delta, duration: duration(duration_ms), easing }))
            }
            (None, None) => {
                if self.duration_ms.is_some() || self.easing.is_some() {
                    return Err(fail("sets a duration or an easing without a delta"));
                }
                Ok(None)
            }
        }
    }
}

fn finite(value: f64, message: &'static str) -> Result<f64, Error> {
    if value.is_finite() { Ok(value) } else { Err(Error::invalid_argument(message)) }
}

fn finite_point(
    point: crate::generated::types::mln_screen_point,
    message: &'static str,
) -> Result<ScreenCoordinate, Error> {
    geo::screen_point(point).map_err(|_| Error::invalid_argument(message))
}

/// The pan's glide.
fn pan(pan: &mln_pan_momentum) -> Result<Option<MomentumGlide<ScreenCoordinate>>, Error> {
    let fields = Fields::new(
        pan.fields,
        MLN_PAN_MOMENTUM_VELOCITY | MLN_PAN_MOMENTUM_DELTA | MLN_PAN_MOMENTUM_DURATION_MS | MLN_PAN_MOMENTUM_EASING,
        "mln_pan_momentum.fields",
    )?;
    GlideFields {
        velocity: fields
            .get(MLN_PAN_MOMENTUM_VELOCITY, pan.velocity)
            .map(|velocity| finite_point(velocity, NOT_FINITE_VELOCITY))
            .transpose()?,
        friction: &pan.friction,
        delta: fields
            .get(MLN_PAN_MOMENTUM_DELTA, pan.delta)
            .map(|delta| finite_point(delta, NOT_FINITE_DELTA))
            .transpose()?,
        duration_ms: fields.get(MLN_PAN_MOMENTUM_DURATION_MS, pan.duration_ms),
        easing: fields.get(MLN_PAN_MOMENTUM_EASING, pan.easing),
    }
    .glide(MomentumFriction::PAN, "pan")
}

/// `options` over the part's `snap`.
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
    let duration_ms = fields.get(MLN_MOMENTUM_SNAP_DURATION_MS, options.duration_ms);
    let valid = positive(snap.spacing)
        && snap.offset.is_finite()
        && snap.tolerance.is_finite()
        && snap.tolerance >= 0.0
        && duration_ms.is_none_or(is_duration_ms);
    if !valid {
        return Err(Error::invalid_argument(
            "a momentum snap needs a positive spacing, a finite offset, a tolerance that isn't negative, and a \
             duration in the native range",
        ));
    }
    if let Some(duration_ms) = duration_ms {
        snap.duration = duration(duration_ms);
    }
    Ok(snap)
}

/// A zoom, bearing or pitch part, with its default friction and snap.
fn anchored(
    part: &mln_anchored_momentum,
    friction: MomentumFriction,
    snap_defaults: MomentumSnap,
    name: &str,
) -> Result<AnchoredMomentum, Error> {
    let fields = Fields::new(
        part.fields,
        MLN_ANCHORED_MOMENTUM_VELOCITY
            | MLN_ANCHORED_MOMENTUM_DELTA
            | MLN_ANCHORED_MOMENTUM_DURATION_MS
            | MLN_ANCHORED_MOMENTUM_EASING
            | MLN_ANCHORED_MOMENTUM_ANCHOR
            | MLN_ANCHORED_MOMENTUM_SNAP,
        "mln_anchored_momentum.fields",
    )?;
    let glide = GlideFields {
        velocity: fields
            .get(MLN_ANCHORED_MOMENTUM_VELOCITY, part.velocity)
            .map(|velocity| finite(velocity, NOT_FINITE_VELOCITY))
            .transpose()?,
        friction: &part.friction,
        delta: fields
            .get(MLN_ANCHORED_MOMENTUM_DELTA, part.delta)
            .map(|delta| finite(delta, NOT_FINITE_DELTA))
            .transpose()?,
        duration_ms: fields.get(MLN_ANCHORED_MOMENTUM_DURATION_MS, part.duration_ms),
        easing: fields.get(MLN_ANCHORED_MOMENTUM_EASING, part.easing),
    };
    Ok(AnchoredMomentum {
        glide: glide.glide(friction, name)?,
        anchor: fields.get(MLN_ANCHORED_MOMENTUM_ANCHOR, part.anchor).map(geo::screen_point).transpose()?,
        snap: fields
            .get(MLN_ANCHORED_MOMENTUM_SNAP, &part.snap)
            .map(|options| snap(options, snap_defaults))
            .transpose()?,
    })
}

/// The momentum that `momentum` sets over the header's defaults.
pub(super) fn momentum(momentum: &mln_gesture_momentum) -> Result<CameraMomentum, Error> {
    Ok(CameraMomentum {
        pan: pan(&momentum.pan)?,
        zoom: anchored(&momentum.zoom, MomentumFriction::ZOOM, MomentumSnap::ZOOM, "zoom")?,
        bearing: anchored(&momentum.bearing, MomentumFriction::BEARING, MomentumSnap::BEARING, "bearing")?,
        pitch: anchored(&momentum.pitch, MomentumFriction::PITCH, MomentumSnap::PITCH, "pitch")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::types::mln_screen_point;

    fn zeroed<T>() -> T {
        // SAFETY: the headers' momentum structs hold numbers only, for which
        // zeros are valid.
        unsafe { core::mem::zeroed() }
    }

    #[test]
    fn unset_settings_take_each_parts_defaults() {
        let resolved = momentum(&zeroed()).unwrap();
        assert_eq!(resolved, CameraMomentum::default());

        let mut options: mln_gesture_momentum = zeroed();
        options.pan.fields = MLN_PAN_MOMENTUM_VELOCITY;
        options.pan.velocity = mln_screen_point { x: 300.0, y: 0.0 };
        options.pan.friction.fields = MLN_MOMENTUM_FRICTION_HALF_LIFE_MS;
        options.pan.friction.half_life_ms = 346.0;
        options.zoom.fields = MLN_ANCHORED_MOMENTUM_SNAP;
        options.bearing.fields = MLN_ANCHORED_MOMENTUM_VELOCITY;
        options.bearing.velocity = -90.0;
        let resolved = momentum(&options).unwrap();
        let friction = MomentumFriction { half_life: duration(346.0), ..MomentumFriction::PAN };
        let velocity = ScreenCoordinate::new(300.0, 0.0);
        assert_eq!(resolved.pan, Some(MomentumGlide::Friction { velocity, friction }));
        assert_eq!(resolved.zoom, AnchoredMomentum { snap: Some(MomentumSnap::ZOOM), ..AnchoredMomentum::default() });
        let friction = MomentumFriction::BEARING;
        assert_eq!(resolved.bearing.glide, Some(MomentumGlide::Friction { velocity: -90.0, friction }));
    }

    #[test]
    fn a_delta_needs_a_duration_and_a_curve_within_the_unit_square() {
        let mut options: mln_gesture_momentum = zeroed();
        options.zoom.fields = MLN_ANCHORED_MOMENTUM_DELTA;
        options.zoom.delta = -1.5;
        let error = momentum(&options).unwrap_err();
        assert_eq!(
            error.message(),
            "the momentum of zoom needs a positive duration in the native range with its delta"
        );

        options.zoom.fields |= MLN_ANCHORED_MOMENTUM_DURATION_MS | MLN_ANCHORED_MOMENTUM_EASING;
        options.zoom.duration_ms = 300.0;
        options.zoom.easing = mln_unit_bezier { x1: 0.2, y1: 2.0, x2: 0.6, y2: 1.0 };
        assert!(momentum(&options).unwrap_err().message().contains("control points are within [0, 1]"));
        options.zoom.easing.y1 = 0.5;
        let easing = MomentumEasing { x1: 0.2, y1: 0.5, x2: 0.6, y2: 1.0 };
        let glide = MomentumGlide::Planned { delta: -1.5, duration: duration(300.0), easing };
        assert_eq!(momentum(&options).unwrap().zoom.glide, Some(glide));

        let mut options: mln_gesture_momentum = zeroed();
        options.pan.fields = MLN_PAN_MOMENTUM_DELTA | MLN_PAN_MOMENTUM_DURATION_MS;
        options.pan.delta = mln_screen_point { x: 30.0, y: -40.0 };
        options.pan.duration_ms = 500.0;
        let delta = ScreenCoordinate::new(30.0, -40.0);
        let glide = MomentumGlide::Planned { delta, duration: duration(500.0), easing: MomentumEasing::DEFAULT };
        assert_eq!(momentum(&options).unwrap().pan, Some(glide));
    }

    #[test]
    fn a_part_glides_one_way_and_reads_only_its_settings() {
        let mut options: mln_gesture_momentum = zeroed();
        options.pitch.fields = MLN_ANCHORED_MOMENTUM_VELOCITY | MLN_ANCHORED_MOMENTUM_DELTA;
        assert_eq!(momentum(&options).unwrap_err().message(), "the momentum of pitch sets both a velocity and a delta");
        options.pitch.fields = MLN_ANCHORED_MOMENTUM_VELOCITY | MLN_ANCHORED_MOMENTUM_DURATION_MS;
        assert!(momentum(&options).unwrap_err().message().contains("a duration or an easing without a delta"));
        options.pitch.fields = MLN_ANCHORED_MOMENTUM_EASING;
        assert!(momentum(&options).unwrap_err().message().contains("a duration or an easing without a delta"));

        // Friction goes unread without a velocity.
        options.pitch.fields = MLN_ANCHORED_MOMENTUM_DELTA | MLN_ANCHORED_MOMENTUM_DURATION_MS;
        options.pitch.delta = 5.0;
        options.pitch.duration_ms = 100.0;
        options.pitch.friction.fields = MLN_MOMENTUM_FRICTION_MAX_SPEED;
        options.pitch.friction.max_speed = -1.0;
        assert!(momentum(&options).is_ok());
    }

    #[test]
    fn settings_outside_their_range_fail() {
        let mut options: mln_gesture_momentum = zeroed();
        options.bearing.fields = MLN_ANCHORED_MOMENTUM_SNAP;
        options.bearing.snap = mln_momentum_snap { fields: MLN_MOMENTUM_SNAP_SPACING, ..zeroed() };
        assert!(momentum(&options).unwrap_err().message().contains("positive spacing"));

        let mut options: mln_gesture_momentum = zeroed();
        options.pitch.fields = MLN_ANCHORED_MOMENTUM_VELOCITY;
        options.pitch.velocity = 10.0;
        options.pitch.friction =
            mln_momentum_friction { fields: MLN_MOMENTUM_FRICTION_MAX_SPEED, max_speed: -1.0, ..zeroed() };
        assert!(momentum(&options).unwrap_err().message().contains("friction of pitch needs a finite minimum speed"));
        options.pitch.friction = mln_momentum_friction { fields: MLN_MOMENTUM_FRICTION_HALF_LIFE_MS, ..zeroed() };
        assert!(momentum(&options).unwrap_err().message().contains("positive half-life"));

        options.pitch.velocity = f64::NAN;
        assert_eq!(momentum(&options).unwrap_err().message(), "gesture momentum velocities must be finite");
    }
}
