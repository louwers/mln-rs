// Ports include/mln/gfx/stencil_mode.hpp

use crate::types::{StencilFunctionType, StencilOpType};

/// `StencilMode::Test = variant<Never, Less, Equal, LessEqual, Greater,
/// NotEqual, GreaterEqual, Always>` (same order). `SimpleTest`s (Never,
/// Always) have no mask; `MaskedTest`s carry one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Test {
    Never,
    Less { mask: u32 },
    Equal { mask: u32 },
    LessEqual { mask: u32 },
    Greater { mask: u32 },
    NotEqual { mask: u32 },
    GreaterEqual { mask: u32 },
    Always,
}

impl Test {
    /// The static `func` of the active alternative.
    #[must_use]
    pub fn func(&self) -> StencilFunctionType {
        match self {
            Test::Never => StencilFunctionType::Never,
            Test::Less { .. } => StencilFunctionType::Less,
            Test::Equal { .. } => StencilFunctionType::Equal,
            Test::LessEqual { .. } => StencilFunctionType::LessEqual,
            Test::Greater { .. } => StencilFunctionType::Greater,
            Test::NotEqual { .. } => StencilFunctionType::NotEqual,
            Test::GreaterEqual { .. } => StencilFunctionType::GreaterEqual,
            Test::Always => StencilFunctionType::Always,
        }
    }

    /// The `mask` member, for `MaskedTest`s only (`requires { test.mask; }`).
    #[must_use]
    pub fn masked(&self) -> Option<u32> {
        match *self {
            Test::Never | Test::Always => None,
            Test::Less { mask }
            | Test::Equal { mask }
            | Test::LessEqual { mask }
            | Test::Greater { mask }
            | Test::NotEqual { mask }
            | Test::GreaterEqual { mask } => Some(mask),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StencilMode {
    pub test: Test,
    pub ref_: i32,
    pub mask: u32,
    pub fail: StencilOpType,
    pub depth_fail: StencilOpType,
    pub pass: StencilOpType,
}

impl StencilMode {
    #[must_use]
    pub fn disabled() -> StencilMode {
        StencilMode {
            test: Test::Always,
            ref_: 0,
            mask: 0,
            fail: StencilOpType::Keep,
            depth_fail: StencilOpType::Keep,
            pass: StencilOpType::Keep,
        }
    }
}

/// C++ value-initializes `StencilMode` members of `Drawable::Impl` (first
/// variant `Never`, zeros, `StencilOpType::Zero`).
impl Default for StencilMode {
    fn default() -> Self {
        StencilMode {
            test: Test::Never,
            ref_: 0,
            mask: 0,
            fail: StencilOpType::Zero,
            depth_fail: StencilOpType::Zero,
            pass: StencilOpType::Zero,
        }
    }
}
