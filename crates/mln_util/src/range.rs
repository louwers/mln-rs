#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, mln_wire::Wire)]
pub struct Range<T> {
    pub min: T,
    pub max: T,
}

impl<T> Range<T> {
    pub const fn new(min: T, max: T) -> Self {
        Range { min, max }
    }
}
