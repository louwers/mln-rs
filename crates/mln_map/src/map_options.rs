// Ports include/mln/map/map_options.hpp and src/mln/map/map_options.cpp.
//
// C++ hides the fields behind a pimpl; the port keeps them private with the
// same builder (`with_*`, by value) and getter methods.

use mln_map_types::{ConstrainMode, MapMode, NorthOrientation, ViewportMode};
use mln_util::size::Size;

/// Holds values for Map options.
#[derive(Clone, Debug)]
pub struct MapOptions {
    map_mode: MapMode,
    constrain_mode: ConstrainMode,
    viewport_mode: ViewportMode,
    orientation: NorthOrientation,
    cross_source_collisions: bool,
    keep_upright_margin: f32,
    capture_rendered_features: bool,
    size: Size,
    pixel_ratio: f32,
    fast_pfor_enabled: bool,
}

impl Default for MapOptions {
    fn default() -> Self {
        MapOptions {
            map_mode: MapMode::Continuous,
            constrain_mode: ConstrainMode::HeightOnly,
            viewport_mode: ViewportMode::Default,
            orientation: NorthOrientation::Upwards,
            cross_source_collisions: true,
            keep_upright_margin: 0.0,
            capture_rendered_features: false,
            size: Size::new(64, 64),
            pixel_ratio: 1.0,
            fast_pfor_enabled: false,
        }
    }
}

impl MapOptions {
    /// `MapOptions()`: the defaults above.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the map rendering mode. By default, it is set to Continuous so
    /// the map will render as data arrives from the network and react
    /// immediately to state changes.
    pub fn with_map_mode(mut self, mode: MapMode) -> Self {
        self.map_mode = mode;
        self
    }

    pub fn map_mode(&self) -> MapMode {
        self.map_mode
    }

    /// Sets the map constrain mode. This can be used to limit the map to
    /// wrap around the globe horizontally. By default, it is set to
    /// HeightOnly.
    pub fn with_constrain_mode(mut self, mode: ConstrainMode) -> Self {
        self.constrain_mode = mode;
        self
    }

    pub fn constrain_mode(&self) -> ConstrainMode {
        self.constrain_mode
    }

    /// Sets the viewport mode. This can be used to flip the vertical
    /// orientation of the map as some devices may use inverted orientation.
    pub fn with_viewport_mode(mut self, mode: ViewportMode) -> Self {
        self.viewport_mode = mode;
        self
    }

    pub fn viewport_mode(&self) -> ViewportMode {
        self.viewport_mode
    }

    /// Enable or disable cross-source symbol collision detection. By
    /// default, it is set to true.
    pub fn with_cross_source_collisions(mut self, enable_collisions: bool) -> Self {
        self.cross_source_collisions = enable_collisions;
        self
    }

    pub fn cross_source_collisions(&self) -> bool {
        self.cross_source_collisions
    }

    /// Sets how far past the vertical a line label kept upright may read
    /// before it is turned over, in degrees. By default, it is 0: a label is
    /// turned over as soon as it would read from right to left, decided anew
    /// on every frame, as in MapLibre GL JS.
    ///
    /// With a margin, a label keeps the reading direction it was last drawn
    /// with until that direction is more than the margin past the vertical of
    /// the screen, and all copies of a label (tiles of two zoom levels, a tile
    /// back from the cache) share it. This keeps labels on lines close to the
    /// vertical, such as the road ahead in a map rotated with the direction of
    /// travel, from turning over and back as the map rotates. A label drawn for
    /// the first time is decided as without the margin, so a still map looks
    /// the same.
    ///
    /// It applies to text placed along lines (`symbol-placement` `line` or
    /// `line-center`) with `text-keep-upright` and `text-rotation-alignment`
    /// `map`, in `MapMode::Continuous` only. Icons, `icon-keep-upright`
    /// included, still turn over as before, and so does the background of an
    /// `icon-text-fit` icon: while a line is within the margin of the vertical,
    /// a label and its icon can face opposite ways.
    ///
    /// `degrees` is clamped to 0 to 45; NaN is ignored. Past 45, labels could
    /// stay drawn more upside down than upright.
    pub fn with_keep_upright_margin(mut self, degrees: f32) -> Self {
        if !degrees.is_nan() {
            self.keep_upright_margin = degrees.clamp(0.0, 45.0);
        }
        self
    }

    /// The keep-upright margin in degrees, 0 by default.
    pub fn keep_upright_margin(&self) -> f32 {
        self.keep_upright_margin
    }

    /// Sets the orientation of the Map. By default, it is set to Upwards.
    pub fn with_north_orientation(mut self, orientation: NorthOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    pub fn north_orientation(&self) -> NorthOrientation {
        self.orientation
    }

    /// Sets the size to resize the map object and renderer backend.
    pub fn with_size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn size(&self) -> Size {
        self.size
    }

    /// Sets the custom pixel ratio. By default, it is set to 1.
    pub fn with_pixel_ratio(mut self, ratio: f32) -> Self {
        self.pixel_ratio = ratio;
        self
    }

    pub fn pixel_ratio(&self) -> f32 {
        self.pixel_ratio
    }

    /// Enable or disable capturing of rendered feature information.
    pub fn with_rendered_feature_info(mut self, capture: bool) -> Self {
        self.capture_rendered_features = capture;
        self
    }

    pub fn rendered_feature_info(&self) -> bool {
        self.capture_rendered_features
    }

    pub fn with_fast_pfor_enabled(mut self, enable: bool) -> Self {
        self.fast_pfor_enabled = enable;
        self
    }

    pub fn fast_pfor_enabled(&self) -> bool {
        self.fast_pfor_enabled
    }
}
