use mln_math::clamp_f64;
use mln_style_types::SourceType;
use mln_util::geo::{LatLng, LatLngBounds};
use mln_util::tileset::{PartialTileset, RasterEncoding, Scheme, VectorEncoding};

use super::Convertible;

/// The member `name` of `value` as `read` reads it, or `None` without one. The error names the
/// member when `read` rejects it.
pub(crate) fn optional_member<'v, T>(
    value: &'v Convertible<'_>,
    name: &str,
    read: impl FnOnce(&Convertible<'v>) -> Option<T>,
) -> Result<Option<T>, String> {
    value.object_member(name).map(|member| read(&member).ok_or_else(|| format!("invalid {name}"))).transpose()
}

/// The tileset members of `value`, a TileJSON document or a style source of type `source_type`,
/// which decides the encodings it may have.
pub(crate) fn convert_tileset(value: &Convertible<'_>, source_type: SourceType) -> Result<PartialTileset, String> {
    let (raster_encoding, vector_encoding) = encoding(value, source_type)?;
    Ok(PartialTileset {
        tiles: optional_member(value, "tiles", strings)?,
        min_zoom: optional_member(value, "minzoom", zoom)?,
        max_zoom: optional_member(value, "maxzoom", zoom)?,
        attribution: optional_member(value, "attribution", Convertible::to_string)?,
        scheme: optional_member(value, "scheme", |scheme| match scheme.to_string()?.as_str() {
            "xyz" => Some(Scheme::XYZ),
            "tms" => Some(Scheme::TMS),
            _ => None,
        })?,
        raster_encoding,
        vector_encoding,
        bounds: optional_member(value, "bounds", bounds)?,
        sparse: optional_member(value, "sparse", Convertible::to_bool)?,
    })
}

/// A zoom level from 0 to 255.
fn zoom(value: &Convertible<'_>) -> Option<u8> {
    value.to_number().filter(|zoom| (0.0..=f32::from(u8::MAX)).contains(zoom)).map(|zoom| zoom as u8)
}

fn strings(value: &Convertible<'_>) -> Option<Vec<String>> {
    if !value.is_array() {
        return None;
    }
    (0..value.array_length()).map(|i| value.array_member(i).to_string()).collect()
}

/// `[west, south, east, north]`, clamped to the world's longitudes and latitudes.
fn bounds(value: &Convertible<'_>) -> Option<LatLngBounds> {
    if !value.is_array() || value.array_length() != 4 {
        return None;
    }
    let [west, south, east, north] = std::array::from_fn(|i| value.array_member(i).to_double());
    let (west, east) = (west?, east?);
    let (south, north) = (clamp_f64(south?, -90.0, 90.0), clamp_f64(north?, -90.0, 90.0));
    if west > east || south > north {
        return None;
    }
    let southwest = LatLng::new(south, west.max(-180.0)).ok()?;
    let northeast = LatLng::new(north, east.min(180.0)).ok()?;
    Some(LatLngBounds::hull(southwest, northeast))
}

/// The `encoding` of a raster-dem or vector source; other sources have none. A `custom`
/// encoding's factors belong to it, so an object that sets `encoding` replaces another's whole,
/// factors included, and factors without an `encoding` beside them do nothing.
fn encoding(
    value: &Convertible<'_>,
    source_type: SourceType,
) -> Result<(Option<RasterEncoding>, Option<VectorEncoding>), String> {
    let Some(encoding) = optional_member(value, "encoding", Convertible::to_string)? else {
        return Ok((None, None));
    };
    let factor =
        |name, default| Ok::<_, String>(optional_member(value, name, Convertible::to_number)?.unwrap_or(default));
    Ok(match (source_type, encoding.as_str()) {
        (SourceType::Vector, "mvt") => (None, Some(VectorEncoding::Mapbox)),
        (SourceType::Vector, "mlt") => (None, Some(VectorEncoding::MLT)),
        (SourceType::RasterDEM, "mapbox") => (Some(RasterEncoding::Mapbox), None),
        (SourceType::RasterDEM, "terrarium") => (Some(RasterEncoding::Terrarium), None),
        (SourceType::RasterDEM, "custom") => (
            Some(RasterEncoding::Custom {
                red_factor: factor("redFactor", 1.0)?,
                green_factor: factor("greenFactor", 1.0)?,
                blue_factor: factor("blueFactor", 1.0)?,
                base_shift: factor("baseShift", 0.0)?,
            }),
            None,
        ),
        (SourceType::Vector, _) => return Err("invalid encoding: a vector source's is 'mvt' or 'mlt'".to_owned()),
        (SourceType::RasterDEM, _) => {
            return Err("invalid encoding: a raster-dem source's is 'mapbox', 'terrarium' or 'custom'".to_owned());
        }
        _ => (None, None),
    })
}
