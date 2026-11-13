/// An error reading a vector tile or converting its features.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A tile, layer or feature fast-mvt can't decode, or a feature's
    /// malformed tags.
    #[error(transparent)]
    Mvt(#[from] fast_mvt::MvtError),
    /// A C++ `std::out_of_range`, with libstdc++'s message.
    #[error("{0}")]
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
