#[derive(Clone, Debug)]
pub struct ProgramParameters {
    // The order is that of the defines in the shaders' keys.
    defines: Vec<(String, String)>,
}

/// The define value: the shortest digits, with a trailing ".0" for a whole
/// number. Only the shaders' keys hold it.
fn to_string_decimal(value: f32) -> String {
    let v = f64::from(value);
    if v.is_finite() && v == v.trunc() { format!("{v:.1}") } else { format!("{v}") }
}

impl ProgramParameters {
    #[must_use]
    pub fn new(pixel_ratio: f32) -> Self {
        let defines = vec![("DEVICE_PIXEL_RATIO".to_owned(), to_string_decimal(pixel_ratio))];
        ProgramParameters { defines }
    }

    /// Each define's name and value (empty for a bare name), in the shaders'
    /// keys' order.
    #[must_use]
    pub fn defines(&self) -> &[(String, String)] {
        &self.defines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defines() {
        let define = |name: &str, value: &str| (name.to_owned(), value.to_owned());
        let p = ProgramParameters::new(1.0);
        assert_eq!(p.defines(), [define("DEVICE_PIXEL_RATIO", "1.0")]);
        let p = ProgramParameters::new(2.5);
        assert_eq!(p.defines(), [define("DEVICE_PIXEL_RATIO", "2.5")]);
    }
}
