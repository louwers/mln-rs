// Ports include/mln/style/conversion/tileset.hpp,
// src/mln/style/conversion/tileset.cpp and the SourceOptions converter of
// src/mln/style/conversion/source_options.cpp.

use mln_util::geo::{LatLng, LatLngBounds};
use mln_util::tileset::{RasterEncoding, Scheme, Tileset, VectorEncoding};

use super::{Converter, Convertible, Error};

/// `SourceOptions`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SourceOptions {
    pub raster_encoding: Option<RasterEncoding>,
    pub vector_encoding: Option<VectorEncoding>,
    /// The style's inline zoom range, which wins over a fetched TileJSON's, as in maplibre-gl-js:
    /// <https://github.com/maplibre/maplibre-gl-js/blob/32e555b2c114d6b60da4acd824d272182c656bbe/src/source/load_tilejson.ts#L39>
    pub minzoom: Option<u8>,
    pub maxzoom: Option<u8>,
}

/// `zoomMember(value, name, error, invalid)`: `Err(())` is C++'s `invalid`.
fn zoom_member(value: &Convertible<'_>, name: &str, error: &mut Error) -> Result<Option<u8>, ()> {
    let Some(member) = value.object_member(name) else {
        return Ok(None);
    };
    match member.to_number() {
        Some(number) if !(number < 0.0 || number > f32::from(u8::MAX)) => Ok(Some(number as u8)),
        _ => {
            error.message = format!("invalid {name}");
            Err(())
        }
    }
}

/// A `custom` raster-dem encoding's number `name`, or `default` without one.
/// `Err(())` is an invalid one, as in `zoom_member`.
fn factor_member(value: &Convertible<'_>, name: &str, default: f32, error: &mut Error) -> Result<f32, ()> {
    let Some(member) = value.object_member(name) else {
        return Ok(default);
    };
    member.to_number().ok_or_else(|| error.message = format!("invalid {name}"))
}

impl Converter for SourceOptions {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        let mut options = SourceOptions::default();
        let mut any = false;

        if let Some(encoding_value) = value.object_member("encoding") {
            match encoding_value.to_string().as_deref() {
                Some("terrarium") => options.raster_encoding = Some(RasterEncoding::Terrarium),
                Some("mapbox") => options.raster_encoding = Some(RasterEncoding::Mapbox),
                Some("custom") => {
                    options.raster_encoding = Some(RasterEncoding::Custom {
                        red_factor: factor_member(value, "redFactor", 1.0, error).ok()?,
                        green_factor: factor_member(value, "greenFactor", 1.0, error).ok()?,
                        blue_factor: factor_member(value, "blueFactor", 1.0, error).ok()?,
                        base_shift: factor_member(value, "baseShift", 0.0, error).ok()?,
                    });
                }
                Some("mvt") => options.vector_encoding = Some(VectorEncoding::Mapbox),
                Some("mlt") => options.vector_encoding = Some(VectorEncoding::MLT),
                _ => {
                    error.message = "invalid encoding - valid types are 'mapbox', 'terrarium' and 'custom' for \
                                     raster sources, 'mvt' and 'mlt' for vector sources"
                        .to_owned();
                    return None;
                }
            }
            any = true;
        }

        options.minzoom = zoom_member(value, "minzoom", error).ok()?;
        options.maxzoom = zoom_member(value, "maxzoom", error).ok()?;
        any = any || options.minzoom.is_some() || options.maxzoom.is_some();

        // gtests-data: C++ `return {};` is an empty optional, not a default
        // SourceOptions (SourceOptions.Basic). Both callers treat the two alike.
        if !any {
            return None;
        }
        Some(options)
    }
}

/// A vector source's `sparse`, `false` when it has none. `Err(())` is an invalid value, with
/// `error` set.
pub(crate) fn sparse_member(value: &Convertible<'_>, error: &mut Error) -> Result<bool, ()> {
    let Some(member) = value.object_member("sparse") else {
        return Ok(false);
    };
    member.to_bool().ok_or_else(|| error.message = "invalid sparse".to_owned())
}

/// `util::clamp(value, min, max)`.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    let lower = if value < max { value } else { max };
    if min < lower { lower } else { min }
}

impl Converter for Tileset {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        let mut result = Tileset::default();

        let Some(tiles) = value.object_member("tiles") else {
            error.message = "source must have tiles".to_owned();
            return None;
        };

        if !tiles.is_array() {
            error.message = "source tiles must be an array".to_owned();
            return None;
        }

        for i in 0..tiles.array_length() {
            let Some(url_template) = tiles.array_member(i).to_string() else {
                error.message = "source tiles member must be a string".to_owned();
                return None;
            };
            result.tiles.push(url_template);
        }

        if let Some(scheme_value) = value.object_member("scheme")
            && scheme_value.to_string().as_deref() == Some("tms")
        {
            result.scheme = Scheme::TMS;
        }

        if let Some(options) = SourceOptions::convert(value, error) {
            if let Some(encoding) = options.raster_encoding {
                result.raster_encoding = Some(encoding);
            }
            if let Some(encoding) = options.vector_encoding {
                result.vector_encoding = Some(encoding);
            }
        }

        if let Some(minzoom_value) = value.object_member("minzoom") {
            match minzoom_value.to_number() {
                Some(minzoom) if !(minzoom < 0.0 || minzoom > f32::from(u8::MAX)) => {
                    result.zoom_range.min = minzoom as u8;
                }
                _ => {
                    error.message = "invalid minzoom".to_owned();
                    return None;
                }
            }
        }

        if let Some(maxzoom_value) = value.object_member("maxzoom") {
            match maxzoom_value.to_number() {
                Some(maxzoom) if !(maxzoom < 0.0 || maxzoom > f32::from(u8::MAX)) => {
                    result.zoom_range.max = maxzoom as u8;
                }
                _ => {
                    error.message = "invalid maxzoom".to_owned();
                    return None;
                }
            }
        }

        if let Some(attribution_value) = value.object_member("attribution") {
            let Some(attribution) = attribution_value.to_string() else {
                error.message = "source attribution must be a string".to_owned();
                return None;
            };
            result.attribution = attribution;
        }

        if let Some(bounds_value) = value.object_member("bounds") {
            if !bounds_value.is_array() || bounds_value.array_length() != 4 {
                error.message = "bounds must be an array with left, bottom, top, and right values".to_owned();
                return None;
            }
            let left = bounds_value.array_member(0).to_double();
            let bottom = bounds_value.array_member(1).to_double();
            let right = bounds_value.array_member(2).to_double();
            let top = bounds_value.array_member(3).to_double();

            let (Some(left), Some(bottom), Some(right), Some(top)) = (left, bottom, right, top) else {
                error.message = "bounds array must contain numeric longitude and latitude values".to_owned();
                return None;
            };

            let bottom = clamp(bottom, -90.0, 90.0);
            let top = clamp(top, -90.0, 90.0);
            if top < bottom {
                error.message = "bounds bottom latitude must be less than or equal to top latitude".to_owned();
                return None;
            }

            if left > right {
                error.message = "bounds left longitude must be less than or equal to right longitude".to_owned();
                return None;
            }
            // `util::max(-180.0, left)`, `util::min(180.0, right)` (no NaN
            // in JSON numbers).
            let left = if left < -180.0 { -180.0 } else { left };
            let right = if right > 180.0 { 180.0 } else { right };
            let (Ok(sw), Ok(ne)) = (LatLng::new(bottom, left), LatLng::new(top, right)) else {
                error.message = "bounds array must contain numeric longitude and latitude values".to_owned();
                return None;
            };
            result.bounds = Some(LatLngBounds::hull(sw, ne));
        }

        Some(result)
    }
}

/// `convertJSON<Tileset>(json, error)` over bytes (see
/// conversion/geojson.rs for the JSON syntax error message).
pub fn convert_json_tileset(json: &[u8], error: &mut Error) -> Option<Tileset> {
    let document: serde_json::Value = match serde_json::from_slice(json) {
        Ok(document) => document,
        Err(e) => {
            error.message = e.to_string();
            return None;
        }
    };
    Tileset::convert(&Convertible::from(&document), error)
}
