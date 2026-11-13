// Ports include/mln/map/map_options.hpp and src/mln/map/map_options.cpp.

use mln_map_types::{ConstrainMode, MapMode, NorthOrientation, ViewportMode};
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
    /// Whether the map captures information about rendered features. Off by
    /// default.
    pub capture_rendered_features: bool,
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
}

impl Default for MapOptions {
    fn default() -> Self {
        MapOptions {
            map_mode: MapMode::Continuous,
            constrain_mode: ConstrainMode::HeightOnly,
            viewport_mode: ViewportMode::Default,
            north_orientation: NorthOrientation::Upwards,
            cross_source_collisions: true,
            capture_rendered_features: false,
            size: Size::new(64, 64),
            pixel_ratio: 1.0,
            fast_pfor_enabled: false,
            unused_style_image_policy: UnusedStyleImagePolicy::Remove,
        }
    }
}
