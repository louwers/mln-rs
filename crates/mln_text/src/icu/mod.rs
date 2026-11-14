//! ICU's Arabic letter shaping (`u_shapeArabic`, icu4c 78.3's `ushape.cpp`),
//! for the BiDi module. Only the code paths for the options it passes are
//! ported.
//!
//! Copyright © 2016 and later Unicode, Inc. and others; portions copyright
//! International Business Machines Corporation and others. Under ICU's
//! license (LICENSE-ICU).

pub(crate) mod ushape;

/// ICU's `UErrorCode`, limited to the values this module produces.
#[expect(non_camel_case_types, reason = "ICU's names for its error codes")]
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
