use std::sync::Arc;

use crate::conversion::to_value::make_style_property;
use crate::conversion::{Convertible, Error};
use crate::expression::{EvaluationContext, Functions};
use crate::feature::Value;
use crate::property::convert::convert_property_value;
use crate::property::table::{Kind, StyleProperty};
use crate::property::value::{PropertyValue, StyleValue as _};
use crate::subdivision_granularity::SubdivisionGranularitySetting;
use crate::types::{ProjectionDefinition, ProjectionType, projection_type_from_name};

/// The name of the built-in registered projection, whose coordinates are
/// lng/lat degrees unchanged.
pub(crate) const IDENTITY_PROJECTION_NAME: &str = "identity";

/// Whether `name`, as a plain `projection.type`, names a registered
/// projection: `identity`, or a name with a colon, which is how an app names
/// the projections it registers.
#[must_use]
pub(crate) fn is_registered_projection_name(name: &str) -> bool {
    name == IDENTITY_PROJECTION_NAME || name.contains(':')
}

/// The style's root `projection` object.
///
/// Its `type` is a built-in projection, a transition or an expression
/// between built-in projections, or the plain name of a registered
/// projection, which the map looks up in its registry. A registered
/// projection draws flat, as Mercator does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Projection {
    type_: PropertyValue<ProjectionDefinition>,
    /// The registered projection that `type` names, with `type_` undefined.
    registered_name: Option<String>,
}

impl Projection {
    #[must_use]
    pub fn new() -> Self {
        Projection::default()
    }

    #[cfg(test)]
    pub(crate) fn set_property(
        &mut self,
        name: &str,
        value: &Convertible<'_>,
        functions: &Arc<Functions>,
    ) -> Option<Error> {
        if name != "type" {
            return Some(Error::new("projection doesn't support this property"));
        }
        let mut error = Error::default();
        if self.set_type_json(value, functions, &mut error).is_none() {
            return Some(error);
        }
        None
    }

    #[must_use]
    pub(crate) fn get_property(&self, name: &str) -> StyleProperty {
        if name != "type" {
            return StyleProperty::default();
        }
        match &self.registered_name {
            Some(registered_name) => StyleProperty::new(Value::String(registered_name.clone()), Kind::Constant),
            None => make_style_property(&self.type_),
        }
    }

    #[must_use]
    pub(crate) fn get_default_type() -> ProjectionDefinition {
        ProjectionDefinition::default()
    }

    /// The built-in projection that `type` gives, or the transition or
    /// expression between built-in projections; undefined when it names a
    /// registered projection ([`registered_name`](Self::registered_name)).
    #[must_use]
    pub fn get_type(&self) -> PropertyValue<ProjectionDefinition> {
        self.type_.clone()
    }

    /// Sets `type` to a built-in projection, or a transition or expression
    /// between built-in projections.
    pub fn set_type(&mut self, type_: PropertyValue<ProjectionDefinition>) {
        self.type_ = type_;
        self.registered_name = None;
    }

    /// The registered projection that `type` names, if it names one.
    #[must_use]
    pub fn registered_name(&self) -> Option<&str> {
        self.registered_name.as_deref()
    }

    /// Sets `type` to the plain name of a registered projection, which the
    /// map looks up in its registry, as a style's `projection` names one and
    /// as GL JS's `map.setProjection({type: name})` sets it. Fails with
    /// [`ErrorKind::InvalidArgument`](crate::error::ErrorKind) for a name
    /// that can't name one: one that isn't `identity` and has no colon, as a
    /// built-in projection's.
    pub fn set_registered_name(&mut self, name: &str) -> Result<(), crate::error::Error> {
        if !is_registered_projection_name(name) {
            return Err(crate::error::Error::invalid_argument(format!(
                "\"{name}\" doesn't name a registered projection: that is identity or a name with a colon"
            )));
        }
        self.type_ = PropertyValue::Undefined;
        self.registered_name = Some(name.to_owned());
        Ok(())
    }

    /// How finely tiles are subdivided for this projection, for the life of
    /// the style: anything that can render a globe at some zoom gets the
    /// globe granularity, so the Mercator hand-off at high zoom reuses the
    /// same tiles.
    #[must_use]
    pub fn get_subdivision_granularity(&self) -> SubdivisionGranularitySetting {
        if self.type_.is_undefined()
            || self.type_.as_constant().is_some_and(|t| *t == ProjectionDefinition::new(ProjectionType::Mercator))
        {
            return SubdivisionGranularitySetting::none();
        }
        SubdivisionGranularitySetting::globe()
    }

    /// The definition at a zoom level, with `globe` resolved; `mercator`
    /// when the style does not set one, and for a registered projection.
    #[must_use]
    pub fn evaluate(&self, zoom: f32) -> ProjectionDefinition {
        let definition = match &self.type_ {
            PropertyValue::Undefined => Projection::get_default_type(),
            PropertyValue::Constant(definition) => *definition,
            PropertyValue::Expression(expression) => {
                expression.evaluate(&EvaluationContext::new(zoom), Projection::get_default_type())
            }
        };
        definition.at_zoom(f64::from(zoom))
    }

    /// Sets `type` from its style JSON `value`, whose expressions parse
    /// against `functions`. `None`, with `error` set, for a value that is no
    /// projection.
    fn set_type_json(&mut self, value: &Convertible<'_>, functions: &Arc<Functions>, error: &mut Error) -> Option<()> {
        if let Some(name) = value.to_string()
            && projection_type_from_name(&name).is_none()
            && is_registered_projection_name(&name)
        {
            self.type_ = PropertyValue::Undefined;
            self.registered_name = Some(name);
            return Some(());
        }
        let type_ = convert_property_value::<ProjectionDefinition>(value, error, false, false, functions)?;
        self.set_type(type_);
        Some(())
    }
}

/// Converts the style's `projection`, whose expressions parse against
/// `functions`.
pub fn convert_projection(
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
    error: &mut Error,
) -> Option<Projection> {
    if !value.is_object() {
        error.message = "projection must be an object".to_owned();
        return None;
    }

    let mut projection = Projection::new();

    if let Some(type_) = value.object_member("type") {
        projection.set_type_json(&type_, functions, error)?;
    }

    Some(projection)
}
