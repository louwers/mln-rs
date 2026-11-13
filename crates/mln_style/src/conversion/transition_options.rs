// Ports include/mln/style/conversion/transition_options.hpp +
// src/mln/style/conversion/transition_options.cpp.

use std::time::Duration;

use super::{Converter, Convertible, Error};
use crate::transition_options::TransitionOptions;

/// `std::chrono::milliseconds(int64_t(number))`: truncated to whole
/// milliseconds; a negative number, below the style spec's minimum of 0,
/// gives zero, as does NaN.
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
