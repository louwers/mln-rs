// Ports src/mln/renderer/property_evaluation_parameters.hpp. It lives in
// mln_style because style/properties.hpp needs it (a cut point of the C++ include cycle).

use std::collections::BTreeSet;
use std::sync::Arc;

use mln_util::chrono::{Duration, TimePoint};
use serde::{Deserialize, Serialize};

use crate::expression::GlobalStateMap;
use crate::map::zoom_history::ZoomHistory;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CrossfadeParameters {
    pub from_scale: f32,
    pub to_scale: f32,
    pub t: f32,
}

#[derive(Clone, Debug)]
pub struct PropertyEvaluationParameters {
    pub z: f32,
    pub now: TimePoint,
    pub zoom_history: ZoomHistory,
    pub default_fade_duration: Duration,

    /// The style's current global state, used to evaluate `global-state` expressions.
    pub global_state: Option<Arc<GlobalStateMap>>,

    /// The global-state properties that changed since the last evaluation;
    /// None means the changed keys are unknown (treat all as changed).
    pub changed_global_state_keys: Option<Arc<BTreeSet<String>>>,

    pub zoom_changed: bool,
    pub layer_changed: bool,
    pub has_crossfade: bool,
    pub global_state_changed: bool,
}

impl PropertyEvaluationParameters {
    /// `explicit PropertyEvaluationParameters(float z)`.
    pub fn new(z: f32) -> Self {
        PropertyEvaluationParameters {
            z,
            now: TimePoint::max(),
            zoom_history: ZoomHistory::default(),
            default_fade_duration: Duration::zero(),
            global_state: None,
            changed_global_state_keys: None,
            zoom_changed: true,
            layer_changed: false,
            has_crossfade: true,
            global_state_changed: false,
        }
    }

    /// `PropertyEvaluationParameters(ZoomHistory, TimePoint, Duration)`.
    pub fn with_zoom_history(zoom_history: ZoomHistory, now: TimePoint, default_fade_duration: Duration) -> Self {
        PropertyEvaluationParameters {
            z: zoom_history.last_zoom,
            now,
            zoom_history,
            default_fade_duration,
            ..PropertyEvaluationParameters::new(0.0)
        }
    }

    pub fn get_crossfade_parameters(&self) -> CrossfadeParameters {
        let fraction = self.z - self.z.floor();
        // std::chrono::duration<float> d = defaultFadeDuration;
        let d = self.default_fade_duration.as_secs_f32();
        let t = if d != 0.0 {
            // std::min((now - lastIntegerZoomTime) / d, 1.0f): both operands
            // go to the common type duration<float, nano>, so the nanosecond
            // count becomes a float and the float seconds are scaled by 1e9.
            let elapsed = (self.now - self.zoom_history.last_integer_zoom_time).count() as f32;
            let ratio = elapsed / (d * 1_000_000_000.0_f32);
            if 1.0_f32 < ratio { 1.0_f32 } else { ratio }
        } else {
            1.0_f32
        };

        if self.z > self.zoom_history.last_integer_zoom {
            CrossfadeParameters { from_scale: 2.0, to_scale: 1.0, t: fraction + (1.0 - fraction) * t }
        } else {
            CrossfadeParameters { from_scale: 0.5, to_scale: 1.0, t: 1.0 - (1.0 - t) * fraction }
        }
    }
}
