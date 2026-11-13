use feature::Value;

fn numeric_value(value: &Value) -> Option<f64> {
    match value {
        Value::Uint(t) => Some(*t as f64),
        Value::Int(t) => Some(*t as f64),
        Value::Double(t) => Some(*t),
        _ => None,
    }
}

fn strip_number(num: f64) -> f64 {
    let decimal_sig_figs = 6.0;
    if num == 0.0 {
        return num;
    }

    // std::max(0.0, x) == (0.0 < x) ? x : 0.0
    let e = decimal_sig_figs - num.abs().log10().ceil();
    let multiplier = 10f64.powf(if 0.0 < e { e } else { 0.0 });

    // We strip precision twice in a row here to avoid cases where stripping
    // an already stripped number will modify its value due to bad floating
    // point precision luck, eg
    // `Math.floor(8.16598 * 100000) / 100000` -> 8.16597
    let first_strip = (num * multiplier).floor() / multiplier;
    (first_strip * multiplier).floor() / multiplier
}

/// Strip precision for numbers, so that we can compare evaluated results with
/// fixtures. Copied from JS expression harness.
pub(crate) fn strip_precision(value: &Value) -> Value {
    if let Some(num) = numeric_value(value) {
        if num == 0.0 {
            return Value::Double(num);
        }
        return Value::Double(strip_number(num));
    }

    match value {
        Value::Array(vec) => Value::Array(vec.iter().map(strip_precision).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), strip_precision(v))).collect()),
        v => v.clone(),
    }
}

/// `tokenize`: split at newlines.
pub(crate) fn tokenize(s: &str) -> Vec<String> {
    s.split('\n').map(str::to_owned).collect()
}

fn which(v: &Value) -> usize {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Uint(_) => 2,
        Value::Int(_) => 3,
        Value::Double(_) => 4,
        Value::String(_) => 5,
        Value::Array(_) => 6,
        Value::Object(_) => 7,
    }
}

/// The number at the start of `s`, after leading whitespace, if there is one.
fn stod(s: &str) -> Option<f64> {
    mln_util::string::float_prefix(s).parse().ok()
}

#[must_use]
pub(crate) fn deep_equal(a: &Value, b: &Value) -> bool {
    let anum = numeric_value(a);
    let bnum = numeric_value(b);
    if let (Some(anum), Some(bnum)) = (anum, bnum) {
        // `stripPrecision(Value) == stripPrecision(Value)`: double compare.
        return strip_precision(&Value::Double(anum)) == strip_precision(&Value::Double(bnum));
    }

    if which(a) != which(b) {
        return false;
    }

    if let (Value::Array(avec), Value::Array(bvec)) = (a, b) {
        if avec.len() != bvec.len() {
            return false;
        }
        return avec.iter().zip(bvec).all(|(x, y)| deep_equal(x, y));
    }

    if let (Value::Object(amap), Value::Object(bmap)) = (a, b) {
        if amap.len() != bmap.len() {
            return false;
        }
        for (k, v) in amap {
            let Some(other) = bmap.get(k) else {
                return false;
            };
            if !deep_equal(v, other) {
                return false;
            }
        }
        return true;
    }

    if a == b {
        return true;
    }

    if let (Value::String(str_a), Value::String(str_b)) = (a, b) {
        if str_a == str_b {
            return true;
        }

        if let (Some(num_a), Some(num_b)) = (stod(str_a), stod(str_b)) {
            return strip_precision(&Value::Double(num_a)) == strip_precision(&Value::Double(num_b));
        }
    }

    false
}

/// How far a color channel in [0, 1] may be from the fixture's. The fixtures
/// hold GL JS's f64 results truncated to 6 significant figures; this port
/// converts Lab and Oklab colors in f32, so a channel can land a couple of
/// 1e-6 away and truncate to a different 6th digit. A difference of one 8-bit
/// step, 1/255, is far outside the tolerance.
pub(crate) const COLOR_CHANNEL_TOLERANCE: f64 = 1e-5;

/// `deep_equal` for an output of a color expression, an `[r, g, b, a]` array
/// or an error, except that channels within `COLOR_CHANNEL_TOLERANCE` of each
/// other are equal.
#[must_use]
pub(crate) fn color_output_equal(a: &Value, b: &Value) -> bool {
    let close = |x: &Value, y: &Value| {
        numeric_value(x).zip(numeric_value(y)).is_some_and(|(x, y)| (x - y).abs() <= COLOR_CHANNEL_TOLERANCE)
    };
    match (a, b) {
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| deep_equal(x, y) || close(x, y))
        }
        _ => deep_equal(a, b),
    }
}

#[must_use]
pub(crate) fn deep_equal_opt(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => deep_equal(a, b),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(channels: [f64; 4]) -> Value {
        Value::Array(channels.into_iter().map(Value::Double).collect())
    }

    #[test]
    fn color_output_equal_absorbs_f32_rounding_only() {
        let expected = color([0.509_169, 0.0, 0.552_985, 0.6]);
        // f32 results that truncate to a different 6th significant digit.
        assert!(!deep_equal(&color([0.509_168_9, 0.0, 0.552_984_9, 0.600_000_02]), &expected));
        assert!(color_output_equal(&color([0.509_168_9, 0.0, 0.552_984_9, 0.600_000_02]), &expected));
        // One 8-bit step off in a single channel.
        assert!(!color_output_equal(&color([0.509_169 + 1.0 / 255.0, 0.0, 0.552_985, 0.6]), &expected));
        assert!(!color_output_equal(&color([0.509_169, 0.0, 0.552_985, 0.6 - 1.0 / 255.0]), &expected));
    }
}
