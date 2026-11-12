use std::fmt;

/// An error reading a vector tile or converting its features.
#[derive(Debug)]
pub enum Error {
    /// A tile, layer or feature fast-mvt can't decode, or a feature's
    /// malformed tags.
    Mvt(fast_mvt::MvtError),
    /// A C++ `std::runtime_error`.
    Runtime(String),
    /// A C++ `std::out_of_range`, with libstdc++'s message.
    OutOfRange(String),
}

impl Error {
    /// `std::vector::at(n)` on a vector of `size` elements: libstdc++'s
    /// `_M_range_check` message.
    #[must_use]
    pub fn vector_range_check(n: usize, size: usize) -> Error {
        Error::OutOfRange(format!("vector::_M_range_check: __n (which is {n}) >= this->size() (which is {size})"))
    }
}

impl From<fast_mvt::MvtError> for Error {
    fn from(e: fast_mvt::MvtError) -> Self {
        Error::Mvt(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Mvt(e) => e.fmt(f),
            Error::Runtime(msg) | Error::OutOfRange(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Mvt(e) => Some(e),
            Error::Runtime(_) | Error::OutOfRange(_) => None,
        }
    }
}
