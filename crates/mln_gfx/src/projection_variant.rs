/// Which projection prelude a shader is compiled against, and whether its
/// draws stand on terrain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ProjectionVariant {
    #[default]
    Mercator,
    Globe,
    /// Mercator, standing on terrain: the draws read the heights of the
    /// ground under their vertices as an attribute, and those that hide
    /// behind the ground drawn in front of them read its depth.
    Terrain,
    /// The globe, standing on terrain, as [`Terrain`](Self::Terrain) stands
    /// on Mercator.
    GlobeTerrain,
}

impl ProjectionVariant {
    /// The variant on the globe if `globe`, standing on terrain if `stands`.
    #[must_use]
    pub fn new(globe: bool, stands: bool) -> ProjectionVariant {
        match (globe, stands) {
            (false, false) => ProjectionVariant::Mercator,
            (true, false) => ProjectionVariant::Globe,
            (false, true) => ProjectionVariant::Terrain,
            (true, true) => ProjectionVariant::GlobeTerrain,
        }
    }

    /// Whether it projects onto the globe.
    #[must_use]
    pub fn is_globe(self) -> bool {
        matches!(self, ProjectionVariant::Globe | ProjectionVariant::GlobeTerrain)
    }

    /// Whether its draws stand on terrain.
    #[must_use]
    pub fn stands(self) -> bool {
        matches!(self, ProjectionVariant::Terrain | ProjectionVariant::GlobeTerrain)
    }
}
