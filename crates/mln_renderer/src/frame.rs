//! What a frame's update hands its encode.

use mln_map_types::{MapDebugOptions, TransformState};
use mln_util::Color;
use mln_util::time::Instant;
use mln_vec_map::VecMap;

use crate::paint_parameters::TransformParameters;
use crate::render_light::EvaluatedLight;
use crate::render_sky::EvaluatedSky;
use crate::render_source::TileOrder;

/// A layer the frame draws.
#[derive(Clone)]
pub(crate) struct FrameLayer {
    /// The id of the layer's render source, `None` for a layer without one.
    pub(crate) source: Option<String>,
    /// The order the layer draws its source's tiles in, `None` for a layer
    /// without tiles.
    pub(crate) tile_order: Option<TileOrder>,
}

/// A frame, as its update leaves it for its encode: the camera, the values
/// that the whole frame draws with, and the layers it draws.
pub(crate) struct Frame {
    pub(crate) transform: TransformParameters,
    pub(crate) debug_options: MapDebugOptions,
    pub(crate) light: EvaluatedLight,
    pub(crate) sky: Option<EvaluatedSky>,
    /// Whether a layer draws in the 3D pass.
    pub(crate) has_3d: bool,
    /// The number of the frame's layers above its lowest 3D layer, which
    /// draw without a depth test; 0 without 3D layers.
    pub(crate) opaque_pass_cutoff: u32,
    /// What the frame clears to: transparent black unless the style's
    /// bottom layer is a solid background.
    pub(crate) background_color: Color,
    pub(crate) symbol_fade_change: f32,
    /// Whether a transition or a fade still runs, which the next frame
    /// continues.
    pub(crate) needs_repaint: bool,
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
    pub(crate) fn new(state: &TransformState, debug_options: MapDebugOptions) -> Self {
        let started = Instant::now();
        Frame {
            transform: TransformParameters::new(state),
            debug_options,
            light: EvaluatedLight::default(),
            sky: None,
            has_3d: false,
            opaque_pass_cutoff: 0,
            background_color: Color::new_unchecked(0.0, 0.0, 0.0, 0.0),
            symbol_fade_change: 0.0,
            needs_repaint: false,
            loaded: false,
            placement_changed: false,
            layers: VecMap::new(),
            started,
            symbols_started: started,
        }
    }
}
