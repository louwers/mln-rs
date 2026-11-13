// Variant order and discriminants follow the C++ enums, except that
// `SourceType` has no `Video` or `Annotations`: mln-rs has neither source.
// `TextVariableAnchorType` is an alias of `SymbolAnchorType` as in C++.

use std::str::FromStr;

use mln_util::ParseEnumError;
use serde::{Deserialize, Serialize};

// TODO: should be in public source.hpp header
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SourceType {
    Vector,
    Raster,
    RasterDEM,
    GeoJSON,
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

/// The `icon-overlap` and `text-overlap` layout properties.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum IconOverlapType {
    /// The property isn't set: `icon-allow-overlap` or `text-allow-overlap`
    /// decides instead.
    #[default]
    Undefined,
    /// Hidden when it collides with any symbol placed before it.
    Never,
    /// Shown even when it collides with symbols placed before it.
    Always,
    /// Hidden only when it collides with a symbol placed before it with
    /// `Never`.
    Cooperative,
}

pub type TextOverlapType = IconOverlapType;

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

/// Implements `as_str` and `FromStr` for a style enum from each value's name
/// in a style. The `match`es are exhaustive, so every value has a name.
macro_rules! style_spec_names {
    ($t:ident { $($variant:ident => $name:literal),* $(,)? }) => {
        impl $t {
            /// The value's name in a style.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }
        }

        impl FromStr for $t {
            type Err = ParseEnumError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($name => Ok(Self::$variant),)*
                    _ => Err(ParseEnumError),
                }
            }
        }
    };
}

style_spec_names!(VisibilityType {
    Visible => "visible",
    None => "none",
});

style_spec_names!(TranslateAnchorType {
    Map => "map",
    Viewport => "viewport",
});

style_spec_names!(RasterResamplingType {
    Linear => "linear",
    Nearest => "nearest",
});

style_spec_names!(ResamplingType {
    Linear => "linear",
    Nearest => "nearest",
});

style_spec_names!(HillshadeIlluminationAnchorType {
    Map => "map",
    Viewport => "viewport",
});

style_spec_names!(RotateAnchorType {
    Map => "map",
    Viewport => "viewport",
});

style_spec_names!(CirclePitchScaleType {
    Map => "map",
    Viewport => "viewport",
});

style_spec_names!(LineCapType {
    Round => "round",
    Butt => "butt",
    Square => "square",
});

style_spec_names!(LineJoinType {
    Miter => "miter",
    Bevel => "bevel",
    Round => "round",
    FakeRound => "fakeround",
    FlipBevel => "flipbevel",
});

style_spec_names!(SymbolPlacementType {
    Point => "point",
    Line => "line",
    LineCenter => "line-center",
});

style_spec_names!(SymbolAnchorType {
    Center => "center",
    Left => "left",
    Right => "right",
    Top => "top",
    Bottom => "bottom",
    TopLeft => "top-left",
    TopRight => "top-right",
    BottomLeft => "bottom-left",
    BottomRight => "bottom-right",
});

style_spec_names!(SymbolZOrderType {
    Auto => "auto",
    ViewportY => "viewport-y",
    Source => "source",
});

style_spec_names!(TextJustifyType {
    Auto => "auto",
    Center => "center",
    Left => "left",
    Right => "right",
});

style_spec_names!(TextTransformType {
    None => "none",
    Uppercase => "uppercase",
    Lowercase => "lowercase",
});

style_spec_names!(TextWritingModeType {
    Horizontal => "horizontal",
    Vertical => "vertical",
});

style_spec_names!(AlignmentType {
    Map => "map",
    Viewport => "viewport",
    Auto => "auto",
});

style_spec_names!(IconTextFitType {
    None => "none",
    Both => "both",
    Width => "width",
    Height => "height",
});

style_spec_names!(IconOverlapType {
    Undefined => "undefined",
    Never => "never",
    Always => "always",
    Cooperative => "cooperative",
});

style_spec_names!(LightAnchorType {
    Map => "map",
    Viewport => "viewport",
});

style_spec_names!(HillshadeMethodType {
    Standard => "standard",
    Basic => "basic",
    Combined => "combined",
    Igor => "igor",
    Multidirectional => "multidirectional",
});

/// Implements `Interpolate` for enums, which aren't interpolated:
/// interpolation returns the first value.
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
    IconOverlapType,
    TextWritingModeType,
    LightAnchorType,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_names() {
        assert_eq!("visible".parse(), Ok(VisibilityType::Visible));
        assert_eq!("none".parse(), Ok(VisibilityType::None));
        assert_eq!("hidden".parse::<VisibilityType>(), Err(ParseEnumError));
        assert_eq!(VisibilityType::None.as_str(), "none");
    }

    #[test]
    fn symbol_anchor_names() {
        assert_eq!("bottom-right".parse(), Ok(SymbolAnchorType::BottomRight));
        assert_eq!(TextVariableAnchorType::TopLeft.as_str(), "top-left");
    }
}
