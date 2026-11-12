// Ports include/mln/style/transition_options.hpp.

use feature::{Value, ValueObject};
use mln_util::chrono::Duration;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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

    /// `TransitionOptions{{duration}}`.
    #[must_use]
    pub fn with_duration(duration: Duration) -> Self {
        TransitionOptions::new(Some(duration), None, true)
    }

    #[must_use]
    pub fn reverse_merge(&self, defaults: &TransitionOptions) -> TransitionOptions {
        TransitionOptions::new(
            if self.duration.is_some() { self.duration } else { defaults.duration },
            if self.delay.is_some() { self.delay } else { defaults.delay },
            self.enable_placement_transitions,
        )
    }

    #[must_use]
    pub fn is_defined(&self) -> bool {
        self.duration.is_some() || self.delay.is_some()
    }

    #[must_use]
    pub fn serialize(&self) -> Value {
        let mut result = ValueObject::new();
        if let Some(duration) = self.duration {
            // duration_cast<milliseconds>(*duration).count() is an int64.
            result.insert("duration".to_owned(), Value::Int(duration.as_millis()));
        }
        if let Some(delay) = self.delay {
            result.insert("delay".to_owned(), Value::Int(delay.as_millis()));
        }
        Value::Object(result)
    }
}
