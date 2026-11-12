// The exceptions vector_tile.hpp lets escape, as one error type.

use std::fmt;

/// An exception thrown by the C++ reader. The variant is the C++ exception
/// type, because callers catch by type: `VectorMVTTileFeature::getGeometries`
/// catches `std::runtime_error` but not the protozero exceptions (which
/// derive from `std::exception` directly) or `std::out_of_range` (a
/// `std::logic_error`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A `protozero::exception`.
    Pbf(protozero::Exception),
    /// `std::runtime_error(msg)`.
    Runtime(String),
    /// `std::out_of_range` from `std::vector::at` (libstdc++ message).
    OutOfRange(String),
}

impl Error {
    /// The C++ `what()` text.
    #[must_use]
    pub fn what(&self) -> &str {
        match self {
            Error::Pbf(e) => e.what(),
            Error::Runtime(msg) | Error::OutOfRange(msg) => msg,
        }
    }

    /// `std::vector::at(n)` on a vector of `size` elements: libstdc++'s
    /// `_M_range_check` message.
    #[must_use]
    pub fn vector_range_check(n: usize, size: usize) -> Error {
        Error::OutOfRange(format!("vector::_M_range_check: __n (which is {n}) >= this->size() (which is {size})"))
    }

    #[must_use]
    pub fn is_runtime_error(&self) -> bool {
        matches!(self, Error::Runtime(_))
    }
}

impl From<protozero::Exception> for Error {
    fn from(e: protozero::Exception) -> Self {
        Error::Pbf(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.what())
    }
}

impl std::error::Error for Error {}

/// `std::vector::at`.
pub(crate) fn at<T>(v: &[T], n: usize) -> Result<&T, Error> {
    v.get(n).ok_or_else(|| Error::vector_range_check(n, v.len()))
}
