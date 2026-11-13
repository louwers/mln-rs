// Ports src/mln/renderer/property_evaluation_parameters.hpp. It lives in
// mln_style because style/properties.hpp needs it (a cut point of the C++ include cycle).

use std::sync::Arc;
use std::time::Duration;

use mln_util::time::Instant;
use mln_vec_map::VecSet;
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
    /// The time to evaluate transitions at; `None` evaluates them as ended.
    pub now: Option<Instant>,
    pub zoom_history: ZoomHistory,
    pub default_fade_duration: Duration,

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
    /// `explicit PropertyEvaluationParameters(float z)`.
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

    /// `PropertyEvaluationParameters(ZoomHistory, TimePoint, Duration)`.
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
