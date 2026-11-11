// Ports include/mln/style/conversion/custom_geometry_source_options.hpp and
// src/mln/style/conversion/custom_geometry_source_options.cpp.

use super::{Converter, Convertible, Error};
use crate::sources::custom_geometry_source::Options;

// The C++ `static_cast<uint8_t/uint16_t>(float)` of out-of-range values is
// UB; the port saturates (`as`).

impl Converter for Options {
    /// The zoom range and tile options; the tile functions are unset.
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        let mut options = Options::default();

        if let Some(minzoom_value) = value.object_member("minzoom") {
            if let Some(n) = minzoom_value.to_number() {
                options.zoom_range.min = n as u8;
            } else {
                error.message = "GeoJSON source minzoom value must be a number".to_owned();
                return None;
            }
        }

        if let Some(maxzoom_value) = value.object_member("maxzoom") {
            if let Some(n) = maxzoom_value.to_number() {
                options.zoom_range.max = n as u8;
            } else {
                error.message = "GeoJSON source maxzoom value must be a number".to_owned();
                return None;
            }
        }

        if let Some(buffer_value) = value.object_member("buffer") {
            if let Some(n) = buffer_value.to_number() {
                options.tile_options.buffer = n as u16;
            } else {
                error.message = "GeoJSON source buffer value must be a number".to_owned();
                return None;
            }
        }

        if let Some(tolerance_value) = value.object_member("tolerance") {
            if let Some(n) = tolerance_value.to_number() {
                options.tile_options.tolerance = f64::from(n);
            } else {
                error.message = "GeoJSON source tolerance value must be a number".to_owned();
                return None;
            }
        }

        if let Some(wrap_value) = value.object_member("wrap") {
            if let Some(b) = wrap_value.to_bool() {
                options.tile_options.wrap = b;
            } else {
                error.message = "CustomGeometrySource TileOptions wrap value must be a boolean".to_owned();
                return None;
            }
        }

        if let Some(clip_value) = value.object_member("clip") {
            if let Some(b) = clip_value.to_bool() {
                options.tile_options.clip = b;
            } else {
                // Quirk kept: the C++ message's typo.
                error.message = "CustomGeometrySource TileOptiosn clip value must be a boolean".to_owned();
                return None;
            }
        }

        Some(options)
    }
}
