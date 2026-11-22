/// Which projection prelude a shader is compiled against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ProjectionVariant {
    #[default]
    Mercator,
    Globe,
    /// Mercator, standing on the terrain's ground: a standing layer's draws
    /// read the height of the ground under them from the DEM.
    Terrain,
    /// The globe, standing on the terrain's ground, as
    /// [`Terrain`](Self::Terrain) stands on Mercator.
    GlobeTerrain,
}

impl ProjectionVariant {
    /// The variant of a projection that is the globe if `globe`, of draws
    /// that stand on the terrain's ground if `stands`.
    #[must_use]
    pub fn new(globe: bool, stands: bool) -> ProjectionVariant {
        match (globe, stands) {
            (false, false) => ProjectionVariant::Mercator,
            (true, false) => ProjectionVariant::Globe,
            (false, true) => ProjectionVariant::Terrain,
            (true, true) => ProjectionVariant::GlobeTerrain,
        }
    }

    /// Whether it projects onto the globe: [`Globe`](Self::Globe) or
    /// [`GlobeTerrain`](Self::GlobeTerrain).
    #[must_use]
    pub fn is_globe(self) -> bool {
        matches!(self, ProjectionVariant::Globe | ProjectionVariant::GlobeTerrain)
    }

    /// Whether its draws stand on the terrain's ground:
    /// [`Terrain`](Self::Terrain) or [`GlobeTerrain`](Self::GlobeTerrain).
    #[must_use]
    pub fn stands(self) -> bool {
        matches!(self, ProjectionVariant::Terrain | ProjectionVariant::GlobeTerrain)
    }
}
