// As for the light (see light.rs), the C++ `ProjectionObserver* observer`
// (src/mln/style/projection_observer.hpp) is replaced by the
// `ProjectionRefMut` guard of `Style::Impl::get_projection_mut`, which calls
// `onProjectionChanged` when the projection's impl changed.

use std::sync::Arc;

use mln_conversion::{Convertible, Error};
use mln_style_types::ProjectionDefinition;

use crate::conversion::convert_property_value;
use crate::conversion_impl::make_style_property;
pub use crate::projection_impl::Impl;
use crate::property_value::PropertyValue;
use crate::style_property::StyleProperty;

/// The style's root `projection` object.
#[derive(Clone, Debug)]
pub struct Projection {
    pub impl_: Arc<Impl>,
}

impl Default for Projection {
    fn default() -> Self {
        Projection::new()
    }
}

impl Projection {
    #[must_use]
    pub fn new() -> Self {
        Projection::from_impl(Arc::new(Impl::default()))
    }

    #[must_use]
    pub fn from_impl(impl_: Arc<Impl>) -> Self {
        Projection { impl_ }
    }

    #[must_use]
    pub fn mutable_impl(&self) -> Impl {
        (*self.impl_).clone()
    }

    pub fn set_property(&mut self, name: &str, value: &Convertible<'_>) -> Option<Error> {
        if name != "type" {
            return Some(Error::new("projection doesn't support this property"));
        }
        let mut error = Error::default();
        let Some(type_) = convert_property_value::<ProjectionDefinition>(value, &mut error, false, false) else {
            return Some(error);
        };
        self.set_type(type_);
        None
    }

    #[must_use]
    pub fn get_property(&self, name: &str) -> StyleProperty {
        if name == "type" {
            return make_style_property(&self.get_type());
        }
        StyleProperty::default()
    }

    #[must_use]
    pub fn get_default_type() -> ProjectionDefinition {
        ProjectionDefinition::default()
    }

    #[must_use]
    pub fn get_type(&self) -> PropertyValue<ProjectionDefinition> {
        self.impl_.type_.clone()
    }

    pub fn set_type(&mut self, type_: PropertyValue<ProjectionDefinition>) {
        let mut impl_ = self.mutable_impl();
        impl_.type_ = type_;
        self.impl_ = Arc::new(impl_);
    }
}
