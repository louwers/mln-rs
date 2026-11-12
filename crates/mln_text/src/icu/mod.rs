//! The parts of ICU (icu4c 78.3, the system ICU the Linux C++ build links)
//! that platform/default/src/mln/text/bidi.cpp needs beyond what the
//! `unicode-bidi` crate provides: `Bidi_Mirroring_Glyph` (`u_charMirror`,
//! from ICU's compiled-in Bidi property data, `ubidi_props.cpp`) and Arabic
//! letter shaping (`u_shapeArabic`, `ushape.cpp`). Only the code paths for
//! the options bidi.cpp passes are ported.
//!
//! Copyright © 2016 and later Unicode, Inc. and others; portions copyright
//! International Business Machines Corporation and others. Under ICU's
//! license (LICENSE-ICU).

pub mod ubidi_props;
#[rustfmt::skip]
mod ubidi_props_data;
pub mod ushape;

/// `UErrorCode` (the values these ports produce).
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UErrorCode {
    U_STRING_NOT_TERMINATED_WARNING,
    U_ZERO_ERROR,
    U_ILLEGAL_ARGUMENT_ERROR,
    U_INDEX_OUTOFBOUNDS_ERROR,
    U_INVALID_STATE_ERROR,
    U_MEMORY_ALLOCATION_ERROR,
    U_BUFFER_OVERFLOW_ERROR,
    U_NO_SPACE_AVAILABLE,
}

impl UErrorCode {
    /// `U_FAILURE(code)`: errors are > `U_ZERO_ERROR`, warnings < 0.
    #[must_use]
    pub fn is_failure(self) -> bool {
        !matches!(self, UErrorCode::U_STRING_NOT_TERMINATED_WARNING | UErrorCode::U_ZERO_ERROR)
    }

    /// `u_errorName(code)`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            UErrorCode::U_STRING_NOT_TERMINATED_WARNING => "U_STRING_NOT_TERMINATED_WARNING",
            UErrorCode::U_ZERO_ERROR => "U_ZERO_ERROR",
            UErrorCode::U_ILLEGAL_ARGUMENT_ERROR => "U_ILLEGAL_ARGUMENT_ERROR",
            UErrorCode::U_INDEX_OUTOFBOUNDS_ERROR => "U_INDEX_OUTOFBOUNDS_ERROR",
            UErrorCode::U_INVALID_STATE_ERROR => "U_INVALID_STATE_ERROR",
            UErrorCode::U_MEMORY_ALLOCATION_ERROR => "U_MEMORY_ALLOCATION_ERROR",
            UErrorCode::U_BUFFER_OVERFLOW_ERROR => "U_BUFFER_OVERFLOW_ERROR",
            UErrorCode::U_NO_SPACE_AVAILABLE => "U_NO_SPACE_AVAILABLE",
        }
    }
}
