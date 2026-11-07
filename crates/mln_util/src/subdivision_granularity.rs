// Ports include/mln/util/subdivision_granularity.hpp.

/// How finely tile geometry is split so the vertex shader can bend it onto
/// the globe; port of GL JS `subdivision_granularity_settings.ts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubdivisionGranularityExpression {
    base_zoom_granularity: u32,
    min_granularity: u32,
}

impl SubdivisionGranularityExpression {
    pub const fn new(base_zoom_granularity: u32, min_granularity: u32) -> Self {
        debug_assert!(min_granularity <= base_zoom_granularity || base_zoom_granularity == 0);
        SubdivisionGranularityExpression { base_zoom_granularity, min_granularity }
    }

    /// The base granularity halves with every zoom level, down to the
    /// minimum and never below 1. (A C++ build without the globe always
    /// returns 1; the port always builds the globe.)
    pub fn get_granularity_for_zoom_level(&self, zoom: u8) -> u32 {
        let base = if zoom < 32 { self.base_zoom_granularity >> zoom } else { 0 };
        base.max(self.min_granularity).max(1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubdivisionGranularitySetting {
    pub fill: SubdivisionGranularityExpression,
    pub line: SubdivisionGranularityExpression,
    pub tile: SubdivisionGranularityExpression,
    pub stencil: SubdivisionGranularityExpression,
    /// Circle vertices per axis: 1, 3, 5 or 7.
    pub circle: u8,
}

impl Default for SubdivisionGranularitySetting {
    fn default() -> Self {
        Self::none()
    }
}

impl SubdivisionGranularitySetting {
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
    pub const fn globe() -> Self {
        SubdivisionGranularitySetting {
            fill: SubdivisionGranularityExpression::new(128, 2),
            line: SubdivisionGranularityExpression::new(512, 0),
            tile: SubdivisionGranularityExpression::new(128, 32),
            stencil: SubdivisionGranularityExpression::new(128, 1),
            circle: 3,
        }
    }
}
