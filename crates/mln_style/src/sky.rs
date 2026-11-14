use std::sync::Arc;

use mln_conversion::{Convertible, Error};

use crate::property_table::{self, SetPropertyError};
pub use crate::sky_impl::{Impl, SkyProperties};
use crate::style_property::StyleProperty;

/// The style's sky.
#[derive(Clone, Debug)]
pub struct Sky {
    pub impl_: Arc<Impl>,
}

impl Default for Sky {
    fn default() -> Self {
        Sky::new()
    }
}

impl Sky {
    #[must_use]
    pub fn new() -> Self {
        Sky::from_impl(Arc::new(Impl::default()))
    }

    #[must_use]
    pub fn from_impl(impl_: Arc<Impl>) -> Self {
        Sky { impl_ }
    }

    /// Sets a property (or `<name>-transition`) from a style JSON value.
    pub fn set_property(&mut self, name: &str, value: &Convertible<'_>) -> Option<Error> {
        match property_table::set_property(&mut self.impl_, name, value) {
            Ok(()) => None,
            Err(SetPropertyError::Invalid(error)) => Some(error),
            Err(SetPropertyError::Unknown) => Some(Error::new("sky doesn't support this property")),
        }
    }

    #[must_use]
    pub fn get_property(&self, name: &str) -> StyleProperty {
        property_table::get_property(&*self.impl_, name)
    }
}
