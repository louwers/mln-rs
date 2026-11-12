// Ports include/mln/style/types.hpp + src/mln/style/types.cpp (the enum
// names, MBGL_DEFINE_ENUM -> mln_util::define_enum!).
//
// Variant order and discriminants follow the C++ enums; `TextVariableAnchorType`
// is an alias of `SymbolAnchorType` as in C++.

use mln_util::define_enum;
use serde::{Deserialize, Serialize};

// TODO: should be in public source.hpp header
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SourceType {
    Vector,
    Raster,
    RasterDEM,
    GeoJSON,
    Video,
    Annotations,
    Image,
    CustomVector,
    CustomMVTVector,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum VisibilityType {
    #[default]
    Visible,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum LineCapType {
    #[default]
    Round,
    Butt,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum LineJoinType {
    #[default]
    Miter,
    Bevel,
    Round,
    // the following two types are for internal use only
    FakeRound,
    FlipBevel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RasterResamplingType {
    #[default]
    Linear,
    Nearest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ResamplingType {
    #[default]
    Linear,
    Nearest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum HillshadeIlluminationAnchorType {
    #[default]
    Map,
    Viewport,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum HillshadeMethodType {
    #[default]
    Standard,
    Combined,
    Igor,
    Multidirectional,
    Basic,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum TranslateAnchorType {
    #[default]
    Map,
    Viewport,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RotateAnchorType {
    #[default]
    Map,
    Viewport,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum CirclePitchScaleType {
    #[default]
    Map,
    Viewport,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SymbolPlacementType {
    #[default]
    Point,
    Line,
    LineCenter,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SymbolZOrderType {
    #[default]
    Auto,
    ViewportY,
    Source,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum AlignmentType {
    #[default]
    Map,
    Viewport,
    Auto,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum TextJustifyType {
    #[default]
    Auto,
    Center,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SymbolAnchorType {
    #[default]
    Center,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

pub type TextVariableAnchorType = SymbolAnchorType;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum TextTransformType {
    #[default]
    None,
    Uppercase,
    Lowercase,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum IconTextFitType {
    #[default]
    None,
    Both,
    Width,
    Height,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum TextWritingModeType {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum LightAnchorType {
    #[default]
    Map,
    Viewport,
}

// --- src/mln/style/types.cpp ---
// Quirk kept: SourceType has no names for RasterDEM and CustomMVTVector
// (the C++ table omits them; toString asserts on them).

define_enum!(
    SourceType,
    [
        (SourceType::Vector, "vector"),
        (SourceType::Raster, "raster"),
        (SourceType::GeoJSON, "geojson"),
        (SourceType::Video, "video"),
        (SourceType::Annotations, "annotations"),
        (SourceType::Image, "image"),
        (SourceType::CustomVector, "customvector"),
    ]
);

define_enum!(VisibilityType, [(VisibilityType::Visible, "visible"), (VisibilityType::None, "none"),]);

define_enum!(TranslateAnchorType, [(TranslateAnchorType::Map, "map"), (TranslateAnchorType::Viewport, "viewport"),]);

define_enum!(
    RasterResamplingType,
    [(RasterResamplingType::Linear, "linear"), (RasterResamplingType::Nearest, "nearest"),]
);

define_enum!(ResamplingType, [(ResamplingType::Linear, "linear"), (ResamplingType::Nearest, "nearest"),]);

define_enum!(
    HillshadeIlluminationAnchorType,
    [(HillshadeIlluminationAnchorType::Map, "map"), (HillshadeIlluminationAnchorType::Viewport, "viewport"),]
);

define_enum!(RotateAnchorType, [(RotateAnchorType::Map, "map"), (RotateAnchorType::Viewport, "viewport"),]);

define_enum!(CirclePitchScaleType, [(CirclePitchScaleType::Map, "map"), (CirclePitchScaleType::Viewport, "viewport"),]);

define_enum!(
    LineCapType,
    [(LineCapType::Round, "round"), (LineCapType::Butt, "butt"), (LineCapType::Square, "square"),]
);

define_enum!(
    LineJoinType,
    [
        (LineJoinType::Miter, "miter"),
        (LineJoinType::Bevel, "bevel"),
        (LineJoinType::Round, "round"),
        (LineJoinType::FakeRound, "fakeround"),
        (LineJoinType::FlipBevel, "flipbevel"),
    ]
);

define_enum!(
    SymbolPlacementType,
    [
        (SymbolPlacementType::Point, "point"),
        (SymbolPlacementType::Line, "line"),
        (SymbolPlacementType::LineCenter, "line-center"),
    ]
);

define_enum!(
    SymbolAnchorType,
    [
        (SymbolAnchorType::Center, "center"),
        (SymbolAnchorType::Left, "left"),
        (SymbolAnchorType::Right, "right"),
        (SymbolAnchorType::Top, "top"),
        (SymbolAnchorType::Bottom, "bottom"),
        (SymbolAnchorType::TopLeft, "top-left"),
        (SymbolAnchorType::TopRight, "top-right"),
        (SymbolAnchorType::BottomLeft, "bottom-left"),
        (SymbolAnchorType::BottomRight, "bottom-right"),
    ]
);

define_enum!(
    SymbolZOrderType,
    [
        (SymbolZOrderType::Auto, "auto"),
        (SymbolZOrderType::ViewportY, "viewport-y"),
        (SymbolZOrderType::Source, "source"),
    ]
);

define_enum!(
    TextJustifyType,
    [
        (TextJustifyType::Auto, "auto"),
        (TextJustifyType::Center, "center"),
        (TextJustifyType::Left, "left"),
        (TextJustifyType::Right, "right"),
    ]
);

define_enum!(
    TextTransformType,
    [
        (TextTransformType::None, "none"),
        (TextTransformType::Uppercase, "uppercase"),
        (TextTransformType::Lowercase, "lowercase"),
    ]
);

define_enum!(
    TextWritingModeType,
    [(TextWritingModeType::Horizontal, "horizontal"), (TextWritingModeType::Vertical, "vertical"),]
);

define_enum!(
    AlignmentType,
    [(AlignmentType::Map, "map"), (AlignmentType::Viewport, "viewport"), (AlignmentType::Auto, "auto"),]
);

define_enum!(
    IconTextFitType,
    [
        (IconTextFitType::None, "none"),
        (IconTextFitType::Both, "both"),
        (IconTextFitType::Width, "width"),
        (IconTextFitType::Height, "height"),
    ]
);

define_enum!(LightAnchorType, [(LightAnchorType::Map, "map"), (LightAnchorType::Viewport, "viewport"),]);

// `Enum<HillshadeMethodType>` is defined next to the hillshade layer in C++
// (src/mln/style/conversion/hillshade_conversions.cpp), with these names.
define_enum!(
    HillshadeMethodType,
    [
        (HillshadeMethodType::Standard, "standard"),
        (HillshadeMethodType::Basic, "basic"),
        (HillshadeMethodType::Combined, "combined"),
        (HillshadeMethodType::Igor, "igor"),
        (HillshadeMethodType::Multidirectional, "multidirectional"),
    ]
);

/// `template <class T> struct Interpolator<T, enable_if_t<is_enum_v<T>>> :
/// Uninterpolated` (include/mln/util/interpolate.hpp): returns the first
/// value.
macro_rules! uninterpolated_enums {
    ($($t:ty),* $(,)?) => {
        $(impl mln_util::interpolate::Interpolate for $t {
            fn interpolate(&self, _to: &Self, _t: f64) -> Self {
                *self
            }
        })*
    };
}

uninterpolated_enums!(
    SourceType,
    VisibilityType,
    LineCapType,
    LineJoinType,
    RasterResamplingType,
    ResamplingType,
    HillshadeIlluminationAnchorType,
    HillshadeMethodType,
    TranslateAnchorType,
    RotateAnchorType,
    CirclePitchScaleType,
    SymbolPlacementType,
    SymbolZOrderType,
    AlignmentType,
    TextJustifyType,
    SymbolAnchorType,
    TextTransformType,
    IconTextFitType,
    TextWritingModeType,
    LightAnchorType,
);

#[cfg(test)]
mod tests {
    use mln_util::enum_::Enum;

    use super::*;

    #[test]
    fn visibility_names() {
        assert_eq!(VisibilityType::to_enum("visible"), Some(VisibilityType::Visible));
        assert_eq!(VisibilityType::to_enum("none"), Some(VisibilityType::None));
        assert_eq!(VisibilityType::to_enum("hidden"), None);
        assert_eq!(VisibilityType::None.to_string(), "none");
    }

    #[test]
    fn symbol_anchor_names() {
        assert_eq!(SymbolAnchorType::to_enum("bottom-right"), Some(SymbolAnchorType::BottomRight));
        assert_eq!(TextVariableAnchorType::TopLeft.to_string(), "top-left");
    }
}
