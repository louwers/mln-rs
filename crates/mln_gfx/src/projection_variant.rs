/// Which projection prelude a shader is compiled against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ProjectionVariant {
    #[default]
    Mercator,
    Globe,
    /// Mercator, standing on the terrain's ground: a standing layer's draws
    /// read the height of the ground under them from the DEM. Never on the
    /// globe, where terrain draws flat.
    Terrain,
}
