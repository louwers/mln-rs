use crate::gfx_types::DrawModeType;

/// The primitives a draw's indexes make.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrawMode {
    Triangles,
    TriangleStrip,
    /// Lines `width` pixels wide, up to the device's widest
    /// ([`Caps::max_line_width`](crate::gpu::Caps::max_line_width)).
    Lines {
        width: f32,
    },
}

impl DrawMode {
    /// The primitive a pipeline of the mode draws.
    #[must_use]
    pub fn primitive(self) -> DrawModeType {
        match self {
            DrawMode::Triangles => DrawModeType::Triangles,
            DrawMode::TriangleStrip => DrawModeType::TriangleStrip,
            DrawMode::Lines { .. } => DrawModeType::Lines,
        }
    }

    /// The width of its lines, 1 for triangles.
    #[must_use]
    pub fn line_width(self) -> f32 {
        match self {
            DrawMode::Lines { width } => width,
            DrawMode::Triangles | DrawMode::TriangleStrip => 1.0,
        }
    }
}
