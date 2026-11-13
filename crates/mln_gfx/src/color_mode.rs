use mln_util::Color;

use crate::types::{ColorBlendEquationType, ColorBlendFactorType};

/// The factors of a linear blend. The equation is the `BlendFunction`
/// variant that holds them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearBlend {
    pub src_factor: ColorBlendFactorType,
    pub dst_factor: ColorBlendFactorType,
}

/// A blend equation and its factors. The variants are in C++'s order, which
/// `which` returns as an index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlendFunction {
    Replace,
    Add(LinearBlend),
    Subtract(LinearBlend),
    ReverseSubtract(LinearBlend),
}

impl BlendFunction {
    #[must_use]
    pub fn which(&self) -> usize {
        match self {
            BlendFunction::Replace => 0,
            BlendFunction::Add(_) => 1,
            BlendFunction::Subtract(_) => 2,
            BlendFunction::ReverseSubtract(_) => 3,
        }
    }

    /// The static `equation` member of the active alternative.
    #[must_use]
    pub fn equation(&self) -> ColorBlendEquationType {
        match self {
            BlendFunction::Replace | BlendFunction::Add(_) => ColorBlendEquationType::Add,
            BlendFunction::Subtract(_) => ColorBlendEquationType::Subtract,
            BlendFunction::ReverseSubtract(_) => ColorBlendEquationType::ReverseSubtract,
        }
    }

    /// The source blend factor (`One` for `Replace`).
    #[must_use]
    pub fn src_factor(&self) -> ColorBlendFactorType {
        match self {
            BlendFunction::Replace => ColorBlendFactorType::One,
            BlendFunction::Add(b) | BlendFunction::Subtract(b) | BlendFunction::ReverseSubtract(b) => b.src_factor,
        }
    }

    /// The destination blend factor (`Zero` for `Replace`).
    #[must_use]
    pub fn dst_factor(&self) -> ColorBlendFactorType {
        match self {
            BlendFunction::Replace => ColorBlendFactorType::Zero,
            BlendFunction::Add(b) | BlendFunction::Subtract(b) | BlendFunction::ReverseSubtract(b) => b.dst_factor,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mask {
    pub r: bool,
    pub g: bool,
    pub b: bool,
    pub a: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorMode {
    pub blend_function: BlendFunction,
    pub blend_color: Color,
    pub mask: Mask,
}

impl ColorMode {
    const ALL: Mask = Mask { r: true, g: true, b: true, a: true };

    #[must_use]
    pub fn disabled() -> ColorMode {
        ColorMode {
            blend_function: BlendFunction::Replace,
            blend_color: Color::default(),
            mask: Mask { r: false, g: false, b: false, a: false },
        }
    }

    #[must_use]
    pub fn unblended() -> ColorMode {
        ColorMode { blend_function: BlendFunction::Replace, blend_color: Color::default(), mask: Self::ALL }
    }

    #[must_use]
    pub fn alpha_blended() -> ColorMode {
        ColorMode {
            blend_function: BlendFunction::Add(LinearBlend {
                src_factor: ColorBlendFactorType::One,
                dst_factor: ColorBlendFactorType::OneMinusSrcAlpha,
            }),
            blend_color: Color::default(),
            mask: Self::ALL,
        }
    }

    #[must_use]
    pub fn additive() -> ColorMode {
        ColorMode {
            blend_function: BlendFunction::Add(LinearBlend {
                src_factor: ColorBlendFactorType::One,
                dst_factor: ColorBlendFactorType::One,
            }),
            blend_color: Color::default(),
            mask: Self::ALL,
        }
    }

    // C++ uses util::hash (boost hash_combine); the value is only
    // used as a pipeline cache key, so any stable combination works.
    #[must_use]
    pub fn hash(&self) -> u64 {
        crate::util_hash::hash(&[
            self.blend_function.which() as u64,
            u64::from(self.blend_color.r.to_bits()),
            u64::from(self.blend_color.g.to_bits()),
            u64::from(self.blend_color.b.to_bits()),
            u64::from(self.blend_color.a.to_bits()),
            u64::from(self.mask.r),
            u64::from(self.mask.g),
            u64::from(self.mask.b),
            u64::from(self.mask.a),
        ])
    }
}
