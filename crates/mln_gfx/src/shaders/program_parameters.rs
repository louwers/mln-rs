use mln_vec_map::VecMap;

#[derive(Clone, Debug)]
pub struct ProgramParameters {
    // The order is that of the defines in the shaders' keys, which a VecMap
    // keeps deterministic.
    defines: VecMap<String, String>,
}

/// The define value: the shortest digits, with a trailing ".0" for a whole
/// number. Only the shaders' keys hold it.
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
        ProgramParameters { defines }
    }

    #[must_use]
    pub fn defines(&self) -> &VecMap<String, String> {
        &self.defines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defines() {
        let p = ProgramParameters::new(1.0, false);
        assert_eq!(p.defines().get("DEVICE_PIXEL_RATIO").map(String::as_str), Some("1.0"));
        assert!(!p.defines().contains_key("OVERDRAW_INSPECTOR"));
        let p = ProgramParameters::new(2.0, true);
        assert!(p.defines().contains_key("OVERDRAW_INSPECTOR"));
    }
}
