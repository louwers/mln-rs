// Ports include/mln/style/expression/image.hpp and
// src/mln/style/expression/image.cpp (incl. `Converter<expression::Image>`).

use feature::{Value as MbglValue, ValueObject};
use mln_conversion::{Convertible, Error};
use serde::{Deserialize, Serialize};

/// `expression::Image`: an image ID plus whether it is available.
// The C++ default constructor leaves `available` uninitialized; here it is
// false.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    image_id: String,
    available: bool,
}

impl Image {
    pub fn new(image_id: impl Into<String>) -> Self {
        Image::with_available(image_id, false)
    }

    pub fn with_available(image_id: impl Into<String>, available: bool) -> Self {
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

/// `Converter<expression::Image>`.
pub fn convert_image(value: &Convertible<'_>, error: &mut Error) -> Option<Image> {
    if value.is_array() {
        let image_parameters = value.array_member(0);
        let image_parameters_length = image_parameters.array_length();
        if image_parameters_length < 1 {
            error.message = "Image has to contain an ID.".to_owned();
            return None;
        }

        let Some(image_id) = image_parameters.array_member(0).to_string() else {
            error.message = "Image has to contain an ID.".to_owned();
            return None;
        };

        Some(Image::with_available(image_id, false))
    } else if let Some(result) = value.to_string() {
        Some(Image::with_available(result, false))
    } else {
        error.message = "Image must be plain string or array type.".to_owned();
        None
    }
}
