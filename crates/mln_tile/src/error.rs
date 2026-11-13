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
    /// The error for index `n` of a vector of `size` elements, with the
    /// message libstdc++'s `std::vector::at` throws.
    #[must_use]
    pub(crate) fn vector_range_check(n: usize, size: usize) -> Error {
        Error::OutOfRange(format!("vector::_M_range_check: __n (which is {n}) >= this->size() (which is {size})"))
    }
}
