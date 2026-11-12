// Ports include/mln/style/conversion/geojson_options.hpp and
// src/mln/style/conversion/geojson_options.cpp.

use std::sync::Arc;

use super::{Converter, Convertible, Error};
use crate::expression::dsl;
use crate::sources::geojson_source::{ClusterProperties, GeoJSONOptions};

// The C++ `static_cast<uint8_t/uint16_t/size_t>(float)` of out-of-range
// values is UB; the port saturates (`as`).

impl Converter for GeoJSONOptions {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
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
                    error.message =
                        "GeoJSON source clusterProperties member must be an array with length of 2".to_owned();
                    return None;
                }
                let Some(map) = dsl::create_expression(&v.array_member(1)) else {
                    error.message = "Failed to convert GeoJSON source clusterProperties map expression".to_owned();
                    return None;
                };
                let reduce = if v.array_member(0).is_array() {
                    dsl::create_expression(&v.array_member(0))
                } else {
                    let Some(reduce_op) = v.array_member(0).to_string() else {
                        error.message =
                            "GeoJSON source clusterProperties member must contain a valid operator".to_owned();
                        return None;
                    };
                    // Reformulate reduce expression to [operator,
                    // ['accumulated'], ['get', key]]. The reason to create
                    // expression via parsing string instead of invoking
                    // function createCompoundExpression is due to expression
                    // type disunity can't be resolved with current logic of
                    // createCompoundExpression
                    let ss = format!(r#"["{reduce_op}", ["accumulated"], ["get", "{k}"]]"#);
                    dsl::create_expression_from_str(&ss)
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
}
