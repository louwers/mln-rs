use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};

// This is not in the type declaration to minimize include dependencies (C++).
static NEXT_ID: AtomicI64 = AtomicI64::new(1);

/// A simple unique identifier.
///
/// `new` takes the next value of a process-wide counter starting at 1;
/// `EMPTY` is 0. Ordering and equality compare the id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimpleIdentity {
    unique_id: i64,
}

impl SimpleIdentity {
    const EMPTY_ID: i64 = 0;

    pub const EMPTY: SimpleIdentity = SimpleIdentity { unique_id: Self::EMPTY_ID };

    /// Returns a new unique identity.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        SimpleIdentity { unique_id: NEXT_ID.fetch_add(1, Ordering::Relaxed) }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.unique_id == Self::EMPTY_ID
    }
}

/// Formats the identity as `{<id>}`.
impl fmt::Display for SimpleIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{{}}}", self.unique_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_are_unique_and_ordered() {
        let a = SimpleIdentity::new();
        let b = SimpleIdentity::new();
        assert!(a < b);
        assert_ne!(a, b);
        assert!(!a.is_empty());
        assert!(SimpleIdentity::EMPTY.is_empty());
        assert_eq!(SimpleIdentity::EMPTY.to_string(), "{0}");
    }
}
