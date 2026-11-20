//! GeoJSON sources from style JSON: their options and their data.

use std::sync::Arc;

use geojson::GeoJson;
use mln_conversion::{Convertible, Error};
use mln_style::expression::Functions;
use mln_style::expression::parsing_context::create_expression;
use mln_style::source_type::SourceData;

use super::{ClusterProperties, GeoJSONOptions, GeoJsonSourceOptions, GeoJsonSourceType};

// The C++ `static_cast<uint8_t/uint16_t/size_t>(float)` of out-of-range
// values is UB; the port saturates (`as`).

/// Converts a GeoJSON source's options, whose `clusterProperties`
/// expressions parse against `functions`.
pub fn convert_geojson_options(
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
    error: &mut Error,
) -> Option<GeoJSONOptions> {
    let mut options = GeoJSONOptions::default();

    if let Some(minzoom_value) = value.object_member("minzoom") {
        if let Some(n) = minzoom_value.to_number() {
            options.minzoom = n as u8;
        } else {
            error.message = "GeoJSON source minzoom value must be a number".to_owned();
            return None;
        }
    }

    if let Some(maxzoom_value) = value.object_member("maxzoom") {
        if let Some(n) = maxzoom_value.to_number() {
            options.maxzoom = n as u8;
        } else {
            error.message = "GeoJSON source maxzoom value must be a number".to_owned();
            return None;
        }
    }

    if let Some(buffer_value) = value.object_member("buffer") {
        if let Some(n) = buffer_value.to_number() {
            options.buffer = n as u16;
        } else {
            error.message = "GeoJSON source buffer value must be a number".to_owned();
            return None;
        }
    }

    if let Some(tolerance_value) = value.object_member("tolerance") {
        if let Some(n) = tolerance_value.to_number() {
            options.tolerance = f64::from(n);
        } else {
            error.message = "GeoJSON source tolerance value must be a number".to_owned();
            return None;
        }
    }

    // Quirk kept: C++ tests `if (toBool(v))`, i.e. that the optional is
    // engaged, so `false` is accepted too.
    if let Some(cluster_value) = value.object_member("cluster") {
        if let Some(b) = cluster_value.to_bool() {
            options.cluster = b;
        } else {
            error.message = "GeoJSON source cluster value must be a boolean".to_owned();
            return None;
        }
    }

    if let Some(cluster_max_zoom_value) = value.object_member("clusterMaxZoom") {
        if let Some(n) = cluster_max_zoom_value.to_number() {
            options.cluster_max_zoom = n as u8;
        } else {
            error.message = "GeoJSON source clusterMaxZoom value must be a number".to_owned();
            return None;
        }
    }

    if let Some(cluster_radius_value) = value.object_member("clusterRadius") {
        if let Some(n) = cluster_radius_value.to_number() {
            options.cluster_radius = n as u16;
        } else {
            error.message = "GeoJSON source clusterRadius value must be a number".to_owned();
            return None;
        }
    }

    if let Some(cluster_min_points_value) = value.object_member("clusterMinPoints") {
        if let Some(n) = cluster_min_points_value.to_number() {
            options.cluster_min_points = n as usize;
        } else {
            error.message = "GeoJSON source clusterMinPoints value must be a number".to_owned();
            return None;
        }
    }

    if let Some(line_metrics_value) = value.object_member("lineMetrics") {
        if let Some(b) = line_metrics_value.to_bool() {
            options.line_metrics = b;
        } else {
            error.message = "GeoJSON source lineMetrics value must be a boolean".to_owned();
            return None;
        }
    }

    if let Some(synchronous_update_value) = value.object_member("synchronousUpdate") {
        if let Some(b) = synchronous_update_value.to_bool() {
            options.synchronous_update = b;
        } else {
            error.message = "GeoJSON source synchronousUpdate value must be a boolean".to_owned();
            return None;
        }
    }

    if let Some(attribution_value) = value.object_member("attribution") {
        let Some(attribution) = attribution_value.to_string() else {
            error.message = "source attribution must be a string".to_owned();
            return None;
        };
        options.attribution = Some(attribution);
    }

    if let Some(cluster_properties) = value.object_member("clusterProperties") {
        if !cluster_properties.is_object() {
            error.message = "GeoJSON source clusterProperties value must be an object".to_owned();
            return None;
        }
        let mut result = ClusterProperties::new();
        debug_assert_eq!(error.message, "");
        cluster_properties.each_member(|k, v| -> Option<Error> {
            // Each property shall be formed as ["key" : [operator,
            // [mapExpression]]] or ["key" : [[operator, ['accumulated'],
            // ['get', key]], [mapExpression]]]
            if !v.is_array() || v.array_length() != 2 {
                error.message = "GeoJSON source clusterProperties member must be an array with length of 2".to_owned();
                return None;
            }
            let Some(map) = create_expression(&v.array_member(1), functions) else {
                error.message = "Failed to convert GeoJSON source clusterProperties map expression".to_owned();
                return None;
            };
            let reduce = if v.array_member(0).is_array() {
                create_expression(&v.array_member(0), functions)
            } else {
                let Some(reduce_op) = v.array_member(0).to_string() else {
                    error.message = "GeoJSON source clusterProperties member must contain a valid operator".to_owned();
                    return None;
                };
                // Reformulate reduce expression to [operator,
                // ['accumulated'], ['get', key]].
                let reduce = serde_json::json!([reduce_op, ["accumulated"], ["get", k]]);
                create_expression(&Convertible::from(&reduce), functions)
            };
            let Some(reduce) = reduce else {
                error.message = "Failed to convert GeoJSON source clusterProperties reduce expression".to_owned();
                return None;
            };
            // `emplace` keeps an existing key.
            result.entry(k.to_owned()).or_insert((Arc::from(map), Arc::from(reduce)));
            None
        });
        if !error.message.is_empty() {
            return None;
        }
        options.cluster_properties = result;
    }

    Some(options)
}

/// A GeoJSON source's data from its style JSON: its options, and its data,
/// built at once, or the URL it loads it from.
pub(crate) fn convert_geojson_source(
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
    error: &mut Error,
) -> Option<SourceData<GeoJsonSourceType>> {
    let Some(data_value) = value.object_member("data") else {
        error.message = "GeoJSON source must have a data value".to_owned();
        return None;
    };

    let mut options = GeoJSONOptions::default_options();
    if let Some(converted) = convert_geojson_options(value, functions, error) {
        options = Arc::new(converted);
    }

    let mut options = GeoJsonSourceOptions::new(options);

    if data_value.is_object() {
        let geojson = data_value.to_geojson(error)?;
        match options.data_builder().build(geojson) {
            Ok(data) => Some(SourceData::with_description(options, data)),
            Err(e) => {
                // A clustered non-point feature fails the style's parse.
                error.message = e.message;
                None
            }
        }
    } else if let Some(url) = data_value.to_string() {
        options.url = Some(url);
        Some(SourceData::new(options))
    } else {
        error.message = "GeoJSON data must be a URL or an object".to_owned();
        None
    }
}

/// Reads GeoJSON from JSON bytes, reporting the `geojson` crate's error
/// (C++: rapidjson's `GetParseError_En(code) + " at offset N"`).
pub(crate) fn convert_json_geojson(json: &[u8], error: &mut Error) -> Option<GeoJson> {
    match serde_json::from_slice(json) {
        Ok(geojson) => Some(geojson),
        Err(e) => {
            error.message = e.to_string();
            None
        }
    }
}
