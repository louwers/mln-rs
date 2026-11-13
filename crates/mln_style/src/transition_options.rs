use std::time::Duration;

use feature::{Value, ValueObject};
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
