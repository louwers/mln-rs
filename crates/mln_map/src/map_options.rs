use mln_map_types::{ConstrainMode, MapMode, NorthOrientation, TerrainSkirtLength, ViewportMode};
use mln_util::size::Size;

/// What a map does with a style image once no layer uses it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnusedStyleImagePolicy {
    /// Remove it from the style, unless the image sets `keep_unused`. A
    /// layer that uses it again reports it missing with
    /// [`MapEvent::StyleImageMissing`](crate::MapEvent::StyleImageMissing).
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
    /// Whether the center stands on the terrain, at its height under the
    /// center, once that has loaded. A camera at rest stands there. A
    /// command eases the altitude toward the ground under the center it is
    /// expected to end on; once that ground has loaded, the command's own
    /// [`CameraOptions::center_altitude`] gives way to it, without a
    /// warning, and while it loads, the command heads for the altitude it
    /// was given. A gesture holds the altitude the camera has when its
    /// fingers go down, lifted over ground that reaches above its eye, and
    /// the commands running then give up theirs. A command that takes the
    /// center from a gesture's momentum eases the altitude as any command
    /// does. A command that runs to its end owning a center or a zoom ends
    /// at them, standing on the ground there. When anything else brings
    /// the camera to rest, such as a gesture's end, a cancel or a command
    /// of the angles alone, a center off the ground moves along the view
    /// onto it, with the eye where it was; where the view meets no drawn
    /// ground, or the bounds or zoom limits refuse that move, the center
    /// stays and stands on the ground as at rest. When this is off, every camera keeps the altitude
    /// it is given, such as the style's `centerAltitude`, and a command or
    /// a gesture whose eye the ground reaches lowers the pitch and zoom
    /// instead. Without terrain it changes nothing. On by default.
    /// [`Map::set_center_clamped_to_ground`] changes it.
    ///
    /// [`CameraOptions::center_altitude`]: mln_map_types::CameraOptions::center_altitude
    ///
    /// [`Map::set_center_clamped_to_ground`]: crate::Map::set_center_clamped_to_ground
    pub center_clamped_to_ground: bool,
    /// How far the skirts of terrain tiles reach down, for the map's
    /// lifetime. `Auto` by default.
    pub terrain_skirt_length: TerrainSkirtLength,
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
            terrain_skirt_length: TerrainSkirtLength::Auto,
        }
    }
}
