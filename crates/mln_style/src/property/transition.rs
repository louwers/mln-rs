use std::sync::Arc;
use std::time::Duration;

use feature::{Value, ValueObject};
use mln_vec_map::VecSet;

use crate::conversion::{Converter, Convertible, Error};
use crate::expression::GlobalStateMap;
use crate::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, mln_wire::Wire)]
pub struct TransitionOptions {
    pub duration: Option<Duration>,
    pub delay: Option<Duration>,
    pub enable_placement_transitions: bool,
}

impl Default for TransitionOptions {
    fn default() -> Self {
        TransitionOptions::new(None, None, true)
    }
}

impl TransitionOptions {
    #[must_use]
    pub fn new(duration: Option<Duration>, delay: Option<Duration>, enable_placement_transitions: bool) -> Self {
        TransitionOptions { duration, delay, enable_placement_transitions }
    }

    #[must_use]
    pub(crate) fn with_duration(duration: Duration) -> Self {
        TransitionOptions::new(Some(duration), None, true)
    }

    #[must_use]
    pub(crate) fn reverse_merge(&self, defaults: &TransitionOptions) -> TransitionOptions {
        TransitionOptions::new(
            if self.duration.is_some() { self.duration } else { defaults.duration },
            if self.delay.is_some() { self.delay } else { defaults.delay },
            self.enable_placement_transitions,
        )
    }

    #[must_use]
    pub(crate) fn is_defined(&self) -> bool {
        self.duration.is_some() || self.delay.is_some()
    }

    #[must_use]
    pub(crate) fn serialize(&self) -> Value {
        let mut result = ValueObject::new();
        // Whole milliseconds, as integers.
        let millis = |duration: Duration| Value::Int(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX));
        if let Some(duration) = self.duration {
            result.insert("duration".to_owned(), millis(duration));
        }
        if let Some(delay) = self.delay {
            result.insert("delay".to_owned(), millis(delay));
        }
        Value::Object(result)
    }
}

/// A change of the style's global state, across which the values that read it
/// transition from what they showed before.
#[derive(Clone, Debug)]
pub struct GlobalStateChange {
    /// The state before the change, which transitions already running keep reading.
    pub prior_state: Arc<GlobalStateMap>,
    /// The keys whose values differ between the prior and the current state.
    pub changed_keys: Arc<VecSet<String>>,
    /// Whether the global state is all that changed, so only the values that
    /// read a changed key start a new transition.
    pub state_only: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TransitionParameters {
    /// When the transitions start; `None` starts them ended.
    pub now: Option<Instant>,
    pub transition: TransitionOptions,
    /// Set when the global state changed since the last transition.
    pub global_state_change: Option<GlobalStateChange>,
}

impl TransitionParameters {
    /// Parameters without a global-state change.
    #[must_use]
    pub fn new(now: Option<Instant>, transition: TransitionOptions) -> Self {
        TransitionParameters { now, transition, global_state_change: None }
    }
}

/// `number` milliseconds, truncated to whole milliseconds; a negative
/// number, below the style spec's minimum of 0, gives zero, as does NaN.
fn whole_milliseconds(number: f32) -> Duration {
    Duration::from_millis(number as u64)
}

impl Converter for TransitionOptions {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<TransitionOptions> {
        if !value.is_object() {
            error.message = "transition must be an object".to_owned();
            return None;
        }

        let mut result = TransitionOptions::default();

        if let Some(duration) = value.object_member("duration") {
            let Some(number) = duration.to_number() else {
                error.message = "duration must be a number".to_owned();
                return None;
            };
            result.duration = Some(whole_milliseconds(number));
        }

        if let Some(delay) = value.object_member("delay") {
            let Some(number) = delay.to_number() else {
                error.message = "delay must be a number".to_owned();
                return None;
            };
            result.delay = Some(whole_milliseconds(number));
        }

        Some(result)
    }
}
