#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, mln_wire::Wire)]
#[repr(u32)]
pub enum MapMode {
    /// continually updating map
    #[default]
    Continuous,
    /// a once-off still image of an arbitrary viewport
    Static,
    /// a once-off still image of a single tile
    Tile,
}

/// We can choose to constrain the map both horizontally or vertically, only
/// vertically e.g. while panning, or screen to the specified bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ConstrainMode {
    None,
    #[default]
    HeightOnly,
    WidthAndHeight,
    Screen,
}

/// Satisfies embedding platforms that requires the viewport coordinate
/// systems to be set according to its standards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ViewportMode {
    #[default]
    Default,
    FlippedY,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TileLodMode {
    /// Default `TileLOD` algorithm
    #[default]
    Default,
    /// Distance-based `TileLOD` algorithm
    Distance,
}

/// How far the skirts that hang from the edges of terrain tiles reach down.
/// Skirts hide the cracks where the edges of neighboring tiles don't meet,
/// and show through where the layers draped over the terrain are
/// translucent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TerrainSkirtLength {
    /// A fifth of a tile's width at the map's zoom, in meters at the
    /// equator.
    #[default]
    Auto,
    /// No skirts.
    None,
}

/// How a map draws its terrain, for the map's lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerrainOptions {
    /// How far the skirts of terrain tiles reach down. `Auto` by default.
    pub skirt_length: TerrainSkirtLength,
    /// The most bytes of GPU memory the textures that the terrain drapes
    /// its layers onto hold, with their mips and the depth and stencil they
    /// draw with. Past it, the drapes of the tiles farthest from the view's
    /// center take fewer texels, and look blurrier; no tile loses its
    /// drape. `None`, the default, sets no limit: the drapes hold what the
    /// view's tiles need.
    pub drape_memory_limit: Option<u64>,
}

impl Default for TerrainOptions {
    fn default() -> Self {
        TerrainOptions { skirt_length: TerrainSkirtLength::Auto, drape_memory_limit: None }
    }
}
