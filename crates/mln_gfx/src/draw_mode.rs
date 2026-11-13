use crate::gfx_types::{DrawModeType, PrimitiveType};

/// A primitive type and its point size or line width. The C++ subclasses
/// (`Points`, `Lines`, `LineStrip`, `Triangles`, `TriangleStrip`) are
/// constructor functions here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawMode {
    pub type_: DrawModeType,
    pub size: f32,
}

impl DrawMode {
    #[expect(non_snake_case, reason = "`DrawMode::Lines(width)` reads as the draw mode's variant")]
    #[must_use]
    pub fn Lines(line_width: f32) -> DrawMode {
        debug_assert!(line_width > 0.0);
        DrawMode { type_: DrawModeType::Lines, size: line_width }
    }

    #[expect(non_snake_case, reason = "`DrawMode::Triangles()` reads as the draw mode's variant")]
    #[must_use]
    pub fn Triangles() -> DrawMode {
        DrawMode { type_: DrawModeType::Triangles, size: 0.0 }
    }

    #[must_use]
    pub fn primitive_type(&self) -> PrimitiveType {
        match self.type_ {
            DrawModeType::Points => PrimitiveType::Point,
            DrawModeType::Lines | DrawModeType::LineLoop | DrawModeType::LineStrip => PrimitiveType::Line,
            _ => PrimitiveType::Triangle,
        }
    }
}
