use mln_util::Color;
use mln_util::string::to_string_double;

use crate::expression::{EvalResult, EvaluationError};

fn stringify_color(r: f64, g: f64, b: f64, a: f64) -> String {
    [r, g, b, a].map(to_string_double).join(", ")
}

pub(crate) fn rgba(r: f64, g: f64, b: f64, a: f64) -> EvalResult<Color> {
    if r < 0.0 || r > 255.0 || g < 0.0 || g > 255.0 || b < 0.0 || b > 255.0 {
        return Err(EvaluationError::new(format!(
            "Invalid rgba value [{}]: 'r', 'g', and 'b' must be between 0 and 255.",
            stringify_color(r, g, b, a)
        )));
    }
    if a < 0.0 || a > 1.0 {
        return Err(EvaluationError::new(format!(
            "Invalid rgba value [{}]: 'a' must be between 0 and 1.",
            stringify_color(r, g, b, a)
        )));
    }
    // C++ `Color(r, g, b, a)` asserts 0..1 per channel (only NaN can get
    // here out of range); the unchecked constructor keeps the release
    // behaviour.
    Ok(Color::new_unchecked((r / 255.0 * a) as f32, (g / 255.0 * a) as f32, (b / 255.0 * a) as f32, a as f32))
}
