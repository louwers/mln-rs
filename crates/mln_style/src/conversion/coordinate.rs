use mln_util::LatLng;

use super::{Converter, Convertible, Error};

impl Converter for LatLng {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<LatLng> {
        if !value.is_array() || value.array_length() < 2 {
            error.message = "coordinate array must contain numeric longitude and latitude values".to_owned();
            return None;
        }
        // Style spec uses GeoJSON convention for specifying coordinates
        let latitude = value.array_member(1).to_double();
        let longitude = value.array_member(0).to_double();

        let (Some(latitude), Some(longitude)) = (latitude, longitude) else {
            error.message = "coordinate array must contain numeric longitude and latitude values".to_owned();
            return None;
        };
        if latitude < -90.0 || latitude > 90.0 {
            error.message = "coordinate latitude must be between -90 and 90".to_owned();
            return None;
        }
        // C++ `LatLng(lat, lon)` throws std::domain_error for a NaN or
        // infinite value, which JSON numbers cannot be.
        match LatLng::new(latitude, longitude) {
            Ok(lat_lng) => Some(lat_lng),
            Err(message) => {
                error.message = message.message;
                None
            }
        }
    }
}
