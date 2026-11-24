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

/// The map's debug overlays, a bit set. Where C++ tests a flag with
/// `lhs & rhs`, which returns a bool there, use `contains`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MapDebugOptions(pub u32);

impl MapDebugOptions {
    pub const NO_DEBUG: MapDebugOptions = MapDebugOptions(0);
    pub const TILE_BORDERS: MapDebugOptions = MapDebugOptions(1 << 1);
    pub const PARSE_STATUS: MapDebugOptions = MapDebugOptions(1 << 2);
    pub const TIMESTAMPS: MapDebugOptions = MapDebugOptions(1 << 3);
    pub const COLLISION: MapDebugOptions = MapDebugOptions(1 << 4);
    pub const OVERDRAW: MapDebugOptions = MapDebugOptions(1 << 5);

    #[must_use]
    pub const fn contains(self, rhs: MapDebugOptions) -> bool {
        (self.0 & rhs.0) != 0
    }
}

impl std::ops::BitOr for MapDebugOptions {
    type Output = MapDebugOptions;
    fn bitor(self, rhs: MapDebugOptions) -> MapDebugOptions {
        MapDebugOptions(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for MapDebugOptions {
    fn bitor_assign(&mut self, rhs: MapDebugOptions) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAndAssign for MapDebugOptions {
    fn bitand_assign(&mut self, rhs: MapDebugOptions) {
        self.0 &= rhs.0;
    }
}

impl std::ops::Not for MapDebugOptions {
    type Output = MapDebugOptions;
    fn not(self) -> MapDebugOptions {
        MapDebugOptions(!self.0)
    }
}
