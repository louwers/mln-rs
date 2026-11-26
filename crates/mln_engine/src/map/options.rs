use mln_style::size::Size;

use crate::camera::{ConstrainMode, NorthOrientation, ViewportMode};

/// What a map does with a style image once no layer uses it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnusedStyleImagePolicy {
    /// Remove it from the style, unless the image sets `keep_unused`. A
    /// layer that uses it again reports it missing with
    /// [`MapEvent::StyleImageMissing`](crate::map::MapEvent::StyleImageMissing).
    #[default]
    Remove,
    /// Keep it in the style.
    Keep,
}

/// How a map renders and lays itself out.
#[derive(Clone, Copy, Debug)]
pub struct MapOptions {
    /// Continuous by default, so the map renders as data arrives from the
    /// network and reacts immediately to state changes.
    pub map_mode: MapMode,
    /// Limits how far the map can move off the world. `HeightOnly` by
    /// default.
    pub constrain_mode: ConstrainMode,
    /// Can flip the vertical orientation of the map, as some devices use an
    /// inverted orientation.
    pub viewport_mode: ViewportMode,
    /// Which way north points on the screen. Upwards by default.
    pub north_orientation: NorthOrientation,
    /// Whether symbols collide across sources. On by default.
    pub cross_source_collisions: bool,
    /// The size of the map and its renderer backend.
    pub size: Size,
    /// 1 by default.
    pub pixel_ratio: f32,
    /// Whether MLT vector tiles decode streams encoded with FastPFOR. Off
    /// by default.
    pub fast_pfor_enabled: bool,
    /// What the map does with a style image once no layer uses it. By
    /// default, it removes the image.
    pub unused_style_image_policy: UnusedStyleImagePolicy,
    /// Whether the center stands on the terrain, as
    /// [`Viewport::center_clamped_to_ground`] describes. On by default.
    ///
    /// [`Viewport::center_clamped_to_ground`]: crate::camera::Viewport::center_clamped_to_ground
    pub center_clamped_to_ground: bool,
    /// How the map draws its terrain, for the map's lifetime.
    pub terrain: TerrainOptions,
}

impl Default for MapOptions {
    fn default() -> Self {
        MapOptions {
            map_mode: MapMode::Continuous,
            constrain_mode: ConstrainMode::HeightOnly,
            viewport_mode: ViewportMode::Default,
            north_orientation: NorthOrientation::Upwards,
            cross_source_collisions: true,
            size: Size::new(64, 64),
            pixel_ratio: 1.0,
            fast_pfor_enabled: false,
            unused_style_image_policy: UnusedStyleImagePolicy::Remove,
            center_clamped_to_ground: true,
            terrain: TerrainOptions::default(),
        }
    }
}

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
