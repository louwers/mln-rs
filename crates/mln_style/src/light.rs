use std::sync::Arc;

use mln_conversion::{Convertible, Error};

use crate::expression::Functions;
pub use crate::light_impl::{Impl, LightProperties};
use crate::property_table::{self, SetPropertyError};
use crate::style_property::StyleProperty;

/// The style's light.
#[derive(Clone, Debug)]
pub struct Light {
    pub impl_: Arc<Impl>,
}

impl Default for Light {
    fn default() -> Self {
        Light::new()
    }
}

impl Light {
    #[must_use]
    pub fn new() -> Self {
        Light::from_impl(Arc::new(Impl::default()))
    }

    #[must_use]
    pub fn from_impl(impl_: Arc<Impl>) -> Self {
        Light { impl_ }
    }

    /// Sets a property (or `<name>-transition`) from a style JSON value,
    /// whose expressions parse against `functions`.
    pub fn set_property(&mut self, name: &str, value: &Convertible<'_>, functions: &Arc<Functions>) -> Option<Error> {
        match property_table::set_property(&mut self.impl_, name, value, functions) {
            Ok(()) => None,
            Err(SetPropertyError::Invalid(error)) => Some(error),
            Err(SetPropertyError::Unknown) => Some(Error::new("light doesn't support this property")),
        }
    }

    #[must_use]
    pub fn get_property(&self, name: &str) -> StyleProperty {
        property_table::get_property(&*self.impl_, name)
    }
}
