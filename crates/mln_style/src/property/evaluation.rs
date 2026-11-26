//! What evaluating a property reads of the frame: the zoom, the time, the
//! global state and the [`ZoomHistory`] behind cross-fades.

use std::sync::Arc;
use std::time::Duration;

use mln_vec_map::VecSet;

use crate::expression::GlobalStateMap;
use crate::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, mln_wire::Wire)]
pub struct CrossfadeParameters {
    pub from_scale: f32,
    pub to_scale: f32,
    pub t: f32,
}

#[derive(Clone, Debug)]
pub struct PropertyEvaluationParameters {
    pub(crate) z: f32,
    /// The time to evaluate transitions at; `None` evaluates them as ended.
    pub now: Option<Instant>,
    pub(crate) zoom_history: ZoomHistory,
    pub(crate) default_fade_duration: Duration,

    /// The style's current global state, used to evaluate `global-state` expressions.
    pub global_state: Option<Arc<GlobalStateMap>>,

    /// The global-state properties that changed since the last evaluation;
    /// None means the changed keys are unknown (treat all as changed).
    pub changed_global_state_keys: Option<Arc<VecSet<String>>>,

    pub zoom_changed: bool,
    pub layer_changed: bool,
    pub has_crossfade: bool,
    pub global_state_changed: bool,
}

impl PropertyEvaluationParameters {
    #[must_use]
    pub fn new(z: f32) -> Self {
        PropertyEvaluationParameters {
            z,
            now: None,
            zoom_history: ZoomHistory::default(),
            default_fade_duration: Duration::ZERO,
            global_state: None,
            changed_global_state_keys: None,
            zoom_changed: true,
            layer_changed: false,
            has_crossfade: true,
            global_state_changed: false,
        }
    }

    #[must_use]
    pub fn with_zoom_history(zoom_history: ZoomHistory, now: Option<Instant>, default_fade_duration: Duration) -> Self {
        PropertyEvaluationParameters {
            z: zoom_history.last_zoom,
            now,
            zoom_history,
            default_fade_duration,
            ..PropertyEvaluationParameters::new(0.0)
        }
    }

    #[must_use]
    pub fn get_crossfade_parameters(&self) -> CrossfadeParameters {
        let fraction = self.z - self.z.floor();
        // The share of the fade since the zoom last crossed an integer.
        let t = match (self.now, self.zoom_history.last_integer_zoom_time) {
            _ if self.default_fade_duration.is_zero() => 1.0,
            (Some(now), Some(last)) => {
                now.saturating_duration_since(last).div_duration_f32(self.default_fade_duration).min(1.0)
            }
            _ => 1.0,
        };

        if self.z > self.zoom_history.last_integer_zoom {
            CrossfadeParameters { from_scale: 2.0, to_scale: 1.0, t: fraction + (1.0 - fraction) * t }
        } else {
            CrossfadeParameters { from_scale: 0.5, to_scale: 1.0, t: 1.0 - (1.0 - t) * fraction }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ZoomHistory {
    pub last_zoom: f32,
    pub(crate) last_floor_zoom: f32,
    pub(crate) last_integer_zoom: f32,
    /// When the zoom last crossed an integer; `None` for long ago.
    pub(crate) last_integer_zoom_time: Option<Instant>,
    pub first: bool,
}

impl Default for ZoomHistory {
    fn default() -> Self {
        ZoomHistory {
            last_zoom: 0.0,
            last_floor_zoom: 0.0,
            last_integer_zoom: 0.0,
            last_integer_zoom_time: None,
            first: true,
        }
    }
}

impl ZoomHistory {
    /// Records zoom `z` at `now`, or at no time in particular for `None`.
    pub fn update(&mut self, z: f32, now: Option<Instant>) -> bool {
        let floor_z = z.floor();

        if self.first {
            self.first = false;
            self.last_integer_zoom = floor_z;
            self.last_integer_zoom_time = None;
            self.last_zoom = z;
            self.last_floor_zoom = floor_z;
            return true;
        }

        if self.last_floor_zoom > floor_z {
            self.last_integer_zoom = floor_z + 1.0;
            self.last_integer_zoom_time = now;
        } else if self.last_floor_zoom < floor_z {
            self.last_integer_zoom = floor_z;
            self.last_integer_zoom_time = now;
        }

        if z != self.last_zoom {
            self.last_zoom = z;
            self.last_floor_zoom = floor_z;
            return true;
        }

        false
    }
}
