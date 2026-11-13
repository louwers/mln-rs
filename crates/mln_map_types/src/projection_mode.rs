/// Holds values for Axonometric rendering. All fields are optional.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProjectionMode {
    /// Set to True to enable axonometric rendering, false otherwise.
    pub axonometric: Option<bool>,
    /// The X skew value represents how much to skew on the x-axis.
    pub x_skew: Option<f64>,
    /// The Y skew value represents how much to skew on the y-axis.
    pub y_skew: Option<f64>,
}

impl ProjectionMode {
    #[must_use]
    pub fn with_axonometric(mut self, o: bool) -> Self {
        self.axonometric = Some(o);
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
