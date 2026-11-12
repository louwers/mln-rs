// Ports expression-test/test_runner_common.cpp: the comparison rules of the
// C++ (and GL JS) expression harness.

use feature::Value;

/// `numericValue<double>`.
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
pub fn strip_precision(value: &Value) -> Value {
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
pub fn tokenize(s: &str) -> Vec<String> {
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

/// `std::stod` (strtod over the whole string; throws if nothing parsed or
/// out of range).
fn stod(s: &str) -> Option<f64> {
    let (value, consumed, erange) = csscolorparser::cstdlib::strtod(s.as_bytes());
    if consumed == 0 || erange { None } else { Some(value) }
}

#[must_use]
pub fn deep_equal(a: &Value, b: &Value) -> bool {
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

/// `deepEqual(const std::optional<Value>&, const std::optional<Value>&)`.
#[must_use]
pub fn deep_equal_opt(a: &Option<Value>, b: &Option<Value>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => deep_equal(a, b),
        (None, None) => true,
        _ => false,
    }
}
