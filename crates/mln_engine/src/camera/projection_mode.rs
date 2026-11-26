/// Whether axonometric rendering is enabled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AxonometricMode {
    /// Axonometric rendering is enabled.
    Enabled,
    /// Axonometric rendering is disabled.
    #[default]
    Disabled,
}

impl AxonometricMode {
    /// Convert to a boolean.
    #[must_use]
    pub fn as_bool(self) -> bool {
        matches!(self, AxonometricMode::Enabled)
    }
}

impl From<bool> for AxonometricMode {
    fn from(value: bool) -> Self {
        if value { AxonometricMode::Enabled } else { AxonometricMode::Disabled }
    }
}

/// Holds values for Axonometric rendering. All fields are optional.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProjectionMode {
    /// Whether to enable axonometric rendering.
    pub axonometric: Option<AxonometricMode>,
    /// The X skew value represents how much to skew on the x-axis.
    pub x_skew: Option<f64>,
    /// The Y skew value represents how much to skew on the y-axis.
    pub y_skew: Option<f64>,
}

impl ProjectionMode {
    #[must_use]
    pub fn with_axonometric(mut self, o: bool) -> Self {
        self.axonometric = Some(o.into());
        self
    }
    #[must_use]
    pub fn with_x_skew(mut self, o: f64) -> Self {
        self.x_skew = Some(o);
        self
    }
    #[must_use]
    pub fn with_y_skew(mut self, o: f64) -> Self {
        self.y_skew = Some(o);
        self
    }
}
