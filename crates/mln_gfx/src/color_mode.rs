use mln_util::Color;

use crate::types::{ColorBlendEquationType, ColorBlendFactorType};

/// The factors of a linear blend. The equation is the `BlendFunction`
/// variant that holds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LinearBlend {
    pub src_factor: ColorBlendFactorType,
    pub dst_factor: ColorBlendFactorType,
}

/// How a fragment's color, S, combines with the target's, D, on every
/// channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendFunction {
    /// S, with no blend.
    Replace,
    /// sS + dD, of the blend's factors s and d.
    Add(LinearBlend),
    /// sS - dD.
    Subtract(LinearBlend),
    /// dD - sS.
    ReverseSubtract(LinearBlend),
    /// min(S, D), with no factors.
    Min,
    /// max(S, D), with no factors.
    Max,
}

impl BlendFunction {
    /// The equation.
    #[must_use]
    pub fn equation(&self) -> ColorBlendEquationType {
        match self {
            BlendFunction::Replace | BlendFunction::Add(_) => ColorBlendEquationType::Add,
            BlendFunction::Subtract(_) => ColorBlendEquationType::Subtract,
            BlendFunction::ReverseSubtract(_) => ColorBlendEquationType::ReverseSubtract,
            BlendFunction::Min => ColorBlendEquationType::Min,
            BlendFunction::Max => ColorBlendEquationType::Max,
        }
    }

    /// The source blend factor: `One` for `Replace`, `Min` and `Max`, whose
    /// equations read none, as WebGPU requires of them.
    #[must_use]
    pub fn src_factor(&self) -> ColorBlendFactorType {
        match self {
            BlendFunction::Replace | BlendFunction::Min | BlendFunction::Max => ColorBlendFactorType::One,
            BlendFunction::Add(b) | BlendFunction::Subtract(b) | BlendFunction::ReverseSubtract(b) => b.src_factor,
        }
    }

    /// The destination blend factor: `Zero` for `Replace`, and `One` for
    /// `Min` and `Max`.
    #[must_use]
    pub fn dst_factor(&self) -> ColorBlendFactorType {
        match self {
            BlendFunction::Replace => ColorBlendFactorType::Zero,
            BlendFunction::Min | BlendFunction::Max => ColorBlendFactorType::One,
            BlendFunction::Add(b) | BlendFunction::Subtract(b) | BlendFunction::ReverseSubtract(b) => b.dst_factor,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

    /// The overdraw inspector's: each fragment adds an eighth of white, so a
    /// pixel drawn eight times or more is white.
    #[must_use]
    pub fn overdraw() -> ColorMode {
        const STEP: f32 = 1.0 / 8.0;
        ColorMode {
            blend_function: BlendFunction::Add(LinearBlend {
                src_factor: ColorBlendFactorType::ConstantColor,
                dst_factor: ColorBlendFactorType::One,
            }),
            blend_color: Color::new_unchecked(STEP, STEP, STEP, 0.0),
            mask: Self::ALL,
        }
    }
}
