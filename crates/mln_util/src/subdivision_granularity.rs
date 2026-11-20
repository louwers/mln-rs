/// How finely tile geometry is split so the vertex shader can bend it onto
/// the globe; port of GL JS `subdivision_granularity_settings.ts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, mln_wire::Wire)]
pub struct SubdivisionGranularityExpression {
    base_zoom_granularity: u32,
    min_granularity: u32,
    surface_granularity: u32,
}

impl SubdivisionGranularityExpression {
    #[must_use]
    pub const fn new(base_zoom_granularity: u32, min_granularity: u32) -> Self {
        debug_assert!(min_granularity <= base_zoom_granularity || base_zoom_granularity == 0);
        SubdivisionGranularityExpression { base_zoom_granularity, min_granularity, surface_granularity: 0 }
    }

    /// Sets the granularity at zoom 0 of a second rule, which halves every
    /// second zoom level instead of every level.
    #[must_use]
    pub const fn with_surface_granularity(self, surface_granularity: u32) -> Self {
        SubdivisionGranularityExpression { surface_granularity, ..self }
    }

    /// The base granularity halves with every zoom level, down to the
    /// minimum and never below 1. The surface granularity halves every
    /// second zoom level: a chord's distance from the sphere falls with the
    /// square of its length, so this keeps that distance about the same in
    /// pixels at every zoom level. A tile gets the finer of the two.
    #[must_use]
    pub fn granularity_for_zoom_level(&self, zoom: u8) -> u32 {
        let base = self.base_zoom_granularity.checked_shr(u32::from(zoom)).unwrap_or(0);
        let surface = self.surface_granularity.checked_shr(u32::from(zoom / 2)).unwrap_or(0);
        base.max(surface).max(self.min_granularity).max(1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, mln_wire::Wire)]
pub struct SubdivisionGranularitySetting {
    pub fill: SubdivisionGranularityExpression,
    pub line: SubdivisionGranularityExpression,
    pub tile: SubdivisionGranularityExpression,
    pub stencil: SubdivisionGranularityExpression,
    /// Circle vertices per axis: 1, 3, 5 or 7.
    pub(crate) circle: u8,
}

impl Default for SubdivisionGranularitySetting {
    fn default() -> Self {
        Self::none()
    }
}

impl SubdivisionGranularitySetting {
    #[must_use]
    pub const fn none() -> Self {
        SubdivisionGranularitySetting {
            fill: SubdivisionGranularityExpression::new(0, 0),
            line: SubdivisionGranularityExpression::new(0, 0),
            tile: SubdivisionGranularityExpression::new(0, 0),
            stencil: SubdivisionGranularityExpression::new(0, 0),
            circle: 1,
        }
    }

    /// GL JS's globe values.
    #[must_use]
    pub const fn globe() -> Self {
        SubdivisionGranularitySetting {
            fill: SubdivisionGranularityExpression::new(128, 2).with_surface_granularity(32),
            line: SubdivisionGranularityExpression::new(512, 0),
            tile: SubdivisionGranularityExpression::new(128, 32),
            stencil: SubdivisionGranularityExpression::new(128, 1).with_surface_granularity(32),
            circle: 3,
        }
    }
}
