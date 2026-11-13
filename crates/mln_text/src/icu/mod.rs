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

pub(crate) mod ubidi_props;
#[rustfmt::skip]
mod ubidi_props_data;
pub(crate) mod ushape;

/// ICU's `UErrorCode`, limited to the values this module produces.
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UErrorCode {
    U_STRING_NOT_TERMINATED_WARNING,
    U_ZERO_ERROR,
    U_BUFFER_OVERFLOW_ERROR,
}

impl UErrorCode {
    /// `U_FAILURE(code)`: errors are > `U_ZERO_ERROR`, warnings < 0.
    #[must_use]
    pub(crate) fn is_failure(self) -> bool {
        !matches!(self, UErrorCode::U_STRING_NOT_TERMINATED_WARNING | UErrorCode::U_ZERO_ERROR)
    }
}
