// Ports include/mln/util/range.hpp.

use serde::{Deserialize, Serialize};

/// `mln::Range<T>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range<T> {
    pub min: T,
    pub max: T,
}

impl<T> Range<T> {
    pub const fn new(min: T, max: T) -> Self {
        Range { min, max }
    }
}
