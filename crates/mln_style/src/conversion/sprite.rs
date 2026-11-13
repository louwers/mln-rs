use super::{Converter, Convertible, Error};
use crate::sprite::Sprite;

impl Converter for Sprite {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Sprite> {
        let Some(id_value) = value.object_member("id") else {
            error.message = "id must be defined for sprite object".to_owned();
            return None;
        };
        // C++ dereferences the optional unchecked (undefined for a non-string
        // id); the port uses an empty string.
        let id = id_value.to_string().unwrap_or_default();

        let Some(url_value) = value.object_member("url") else {
            error.message = "url must be defined for sprite object".to_owned();
            return None;
        };
        let sprite_url = url_value.to_string().unwrap_or_default();

        Some(Sprite::new(id, sprite_url))
    }
}
