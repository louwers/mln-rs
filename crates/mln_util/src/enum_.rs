// The module is `enum_` because `enum` is a Rust keyword.

/// The string names of an enum, as defined with `define_enum!`.
pub trait Enum: Sized + Copy + 'static {
    /// The value's name. Enums defined with `define_enum!` panic in debug
    /// builds when the value has no name, as C++ asserts.
    fn to_string(self) -> &'static str;

    /// The first value whose name equals `s`.
    fn to_enum(s: &str) -> Option<Self>;
}

/// `define_enum!(T, [(T::A, "a"), ...])` implements [`Enum`] for `T` by
/// linear search through the name table, in table order.
#[macro_export]
macro_rules! define_enum {
    ($t:ty, [$(($value:expr, $name:expr)),* $(,)?]) => {
        impl $crate::enum_::Enum for $t {
            fn to_string(self) -> &'static str {
                const NAMES: &[($t, &str)] = &[$(($value, $name)),*];
                match NAMES.iter().find(|v| v.0 == self) {
                    Some(v) => v.1,
                    None => {
                        debug_assert!(false, "enum value without a name");
                        ""
                    }
                }
            }

            fn to_enum(s: &str) -> Option<Self> {
                const NAMES: &[($t, &str)] = &[$(($value, $name)),*];
                NAMES.iter().find(|v| v.1 == s).map(|v| v.0)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::Enum;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Sample {
        A,
        B,
    }

    crate::define_enum!(Sample, [(Sample::A, "a"), (Sample::B, "b")]);

    #[test]
    fn round_trip() {
        assert_eq!(Sample::A.to_string(), "a");
        assert_eq!(Sample::to_enum("b"), Some(Sample::B));
        assert_eq!(Sample::to_enum("c"), None);
    }
}
