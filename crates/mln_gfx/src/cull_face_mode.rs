// Ports src/mln/gfx/cull_face_mode.hpp

use crate::types::{CullFaceSideType, CullFaceWindingType};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CullFaceMode {
    pub enabled: bool,
    pub side: CullFaceSideType,
    pub winding: CullFaceWindingType,
}

impl CullFaceMode {
    #[must_use]
    pub fn disabled() -> CullFaceMode {
        CullFaceMode { enabled: false, side: CullFaceSideType::Back, winding: CullFaceWindingType::CounterClockwise }
    }

    #[must_use]
    pub fn back_ccw() -> CullFaceMode {
        CullFaceMode { enabled: true, side: CullFaceSideType::Back, winding: CullFaceWindingType::CounterClockwise }
    }
}
