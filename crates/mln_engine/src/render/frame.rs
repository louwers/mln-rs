//! What a frame's update hands its encode.

use mln_map_types::View;
use mln_style::color::Color;
use mln_style::time::Instant;
use mln_vec_map::VecMap;

use crate::render::light::EvaluatedLight;
use crate::render::paint_parameters::TransformParameters;
use crate::render::sky::EvaluatedSky;

/// A layer the frame draws.
#[derive(Clone)]
pub(crate) struct FrameLayer {
    /// The id of the layer's render source, `None` for a layer without one.
    pub(crate) source: Option<String>,
}

/// A frame, as its update leaves it for its encode: the camera, the values
/// that the whole frame draws with, and the layers it draws.
pub(crate) struct Frame {
    pub(crate) transform: TransformParameters,
    /// Whether a gesture or an animation moves the camera.
    pub(crate) camera_moving: bool,
    pub(crate) light: EvaluatedLight,
    pub(crate) sky: Option<EvaluatedSky>,
    /// Whether a layer draws in the 3D pass.
    pub(crate) has_3d: bool,
    /// Whether the frame draws terrain, which the layers that drape over it
    /// don't draw onto, and under which the other 2D layers draw without
    /// depth.
    pub(crate) terrain: bool,
    /// What the frame clears to: transparent black unless the style's
    /// bottom layer is a solid background.
    pub(crate) background_color: Color,
    pub(crate) symbol_fade_change: f32,
    /// Whether a transition or a fade still runs, which the next frame
    /// continues.
    pub(crate) needs_repaint: bool,
    /// When, without `needs_repaint`, the map changes next on its own: a
    /// symbol placement that isn't due yet.
    pub(crate) repaint_at: Option<Instant>,
    /// Whether every source, image and tile the frame needs had loaded.
    pub(crate) loaded: bool,
    pub(crate) placement_changed: bool,
    /// The layers the frame draws, by their index in the drawing order.
    pub(crate) layers: VecMap<u32, FrameLayer>,
    /// When the frame's update started.
    pub(crate) started: Instant,
    /// When the frame's update started placing symbols.
    pub(crate) symbols_started: Instant,
}

impl Frame {
    /// A frame of `state` that draws no layers, started now.
    pub(crate) fn new(state: &View, camera_moving: bool) -> Self {
        let started = Instant::now();
        Frame {
            transform: TransformParameters::new(state),
            camera_moving,
            light: EvaluatedLight::default(),
            sky: None,
            has_3d: false,
            terrain: false,
            background_color: Color::new_unchecked(0.0, 0.0, 0.0, 0.0),
            symbol_fade_change: 0.0,
            needs_repaint: false,
            repaint_at: None,
            loaded: false,
            placement_changed: false,
            layers: VecMap::new(),
            started,
            symbols_started: started,
        }
    }
}
