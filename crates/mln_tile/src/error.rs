/// An error reading tile data: a layer, a feature or a feature's geometry
/// that can't be decoded.
#[derive(Debug, thiserror::Error)]
pub enum TileError {
    /// A tile, layer or feature fast-mvt can't decode, or a feature's
    /// malformed tags.
    #[error(transparent)]
    Mvt(#[from] fast_mvt::MvtError),
    /// A feature's geometry that can't be decoded.
    #[error("{0}")]
    Geometry(String),
    /// An index past the end, with libstdc++'s message for it.
    #[error(transparent)]
    OutOfRange(#[from] mln_style::feature::OutOfRange),
}

impl TileError {
    /// The error for index `n` of a vector of `size` elements, with the
    /// message libstdc++'s `std::vector::at` throws.
    #[must_use]
    pub(crate) fn vector_range_check(n: usize, size: usize) -> TileError {
        TileError::OutOfRange(mln_style::feature::OutOfRange { index: n, len: size })
    }
}

impl From<TileError> for mln_style::error::Error {
    fn from(error: TileError) -> Self {
        mln_style::error::Error::native(error.to_string())
    }
}
