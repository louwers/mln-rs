use feature::{Value as MbglValue, ValueObject};

use crate::conversion::{Convertible, Error};

/// An image ID plus whether it is available.
#[derive(Clone, Debug, Default, PartialEq, Eq, mln_wire::Wire)]
pub struct Image {
    image_id: String,
    available: bool,
}

impl Image {
    pub fn new(image_id: impl Into<String>) -> Self {
        Image::with_available(image_id, false)
    }

    pub(crate) fn with_available(image_id: impl Into<String>, available: bool) -> Self {
        Image { image_id: image_id.into(), available }
    }

    #[must_use]
    pub fn to_value(&self) -> MbglValue {
        let mut object = ValueObject::new();
        object.insert("name".to_owned(), MbglValue::String(self.image_id.clone()));
        object.insert("available".to_owned(), MbglValue::Bool(self.available));
        MbglValue::Object(object)
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.image_id
    }

    #[must_use]
    pub fn is_available(&self) -> bool {
        self.available
    }

    #[must_use]
    pub fn empty(&self) -> bool {
        self.image_id.is_empty()
    }
}

/// Converts an image ID. An array is an expression, which the property value
/// conversion parses before this.
pub fn convert_image(value: &Convertible<'_>, error: &mut Error) -> Option<Image> {
    if let Some(result) = value.to_string() {
        Some(Image::new(result))
    } else {
        error.message = "value must be a string".to_owned();
        None
    }
}

/// Images don't interpolate: the result is `self`.
impl crate::interpolate::Interpolate for Image {
    fn interpolate(&self, _to: &Self, _t: f64) -> Self {
        self.clone()
    }
}
