use std::fmt;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

// This is not in the type declaration to minimize include dependencies (C++).
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A simple unique identifier.
///
/// `new` takes the next value of a process-wide counter starting at 1.
/// Ordering and equality compare the id. Use `Option<SimpleIdentity>` for an
/// identity that may be absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct SimpleIdentity {
    unique_id: NonZeroU64,
}

impl SimpleIdentity {
    /// Returns a new unique identity.
    pub(crate) fn new() -> Self {
        SimpleIdentity {
            unique_id: NonZeroU64::new(NEXT_ID.fetch_add(1, Ordering::Relaxed)).unwrap_or(NonZeroU64::MIN),
        }
    }

    /// The identity's number, which grows with each new identity.
    #[must_use]
    pub(crate) fn get(self) -> u64 {
        self.unique_id.get()
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
        assert_eq!(a.to_string(), format!("{{{}}}", a.unique_id));
    }
}
