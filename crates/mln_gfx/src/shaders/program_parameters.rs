use std::cell::OnceCell;

use mln_vec_map::VecMap;

#[derive(Clone, Debug)]
pub struct ProgramParameters {
    // C++ uses an unordered_map; the order only affects the defines string
    // and its hash (a cache key), so a VecMap keeps it deterministic.
    defines: VecMap<String, String>,
    defines_hash: u64,
    defines_str: OnceCell<String>,
}

/// The define value: the shortest digits, with a trailing ".0" for a whole
/// number. Only the defines string (a cache key) holds it.
fn to_string_decimal(value: f32) -> String {
    let v = f64::from(value);
    if v.is_finite() && v == v.trunc() { format!("{v:.1}") } else { format!("{v}") }
}

impl ProgramParameters {
    #[must_use]
    pub fn new(pixel_ratio: f32, overdraw: bool) -> Self {
        let mut defines = VecMap::new();
        defines.insert("DEVICE_PIXEL_RATIO".to_owned(), to_string_decimal(pixel_ratio));
        if overdraw {
            defines.insert("OVERDRAW_INSPECTOR".to_owned(), String::new());
        }
        let mut params = ProgramParameters { defines, defines_hash: 0, defines_str: OnceCell::new() };
        params.defines_hash = crate::util_hash::hash_str(params.get_defines_string());
        params
    }

    /// Built-in shader preprocessor defines as `#define KEY VALUE` lines.
    pub fn get_defines_string(&self) -> &str {
        self.defines_str.get_or_init(|| {
            let mut s = String::with_capacity(self.defines.len() * 32);
            for (key, value) in &self.defines {
                s.push_str("#define ");
                s.push_str(key);
                if !value.is_empty() {
                    s.push(' ');
                    s.push_str(value);
                }
                s.push('\n');
            }
            s
        })
    }

    pub fn get_defines(&self) -> &VecMap<String, String> {
        &self.defines
    }

    pub fn get_defines_hash(&self) -> u64 {
        self.defines_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defines() {
        let p = ProgramParameters::new(1.0, false);
        assert_eq!(p.get_defines_string(), "#define DEVICE_PIXEL_RATIO 1.0\n");
        let p = ProgramParameters::new(2.0, true);
        assert!(p.get_defines().contains_key("OVERDRAW_INSPECTOR"));
    }
}
