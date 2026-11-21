use std::fmt::Write;
use std::time::Duration;

use feature::Value;
use mln_render::RenderingStats;
use mln_style::conversion::{Convertible, convert_source};
use mln_style::{Edit, Layer};
use mln_util::Color;
use mln_util::constants::EXTENT;
use mln_util::time::Instant;

use crate::map::Map;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderingStatsViewOptions {
    /// The least time between two updates of the text.
    pub(crate) update_interval: Duration,
    /// Also show the totals and the update counts.
    pub(crate) verbose: bool,
    pub(crate) text_color: Color,
    pub(crate) text_size: f32,
}

impl Default for RenderingStatsViewOptions {
    fn default() -> Self {
        RenderingStatsViewOptions {
            update_interval: Duration::from_millis(250),
            verbose: false,
            text_color: Color::red(),
            text_size: 4.0,
        }
    }
}

/// Keeps a layer of the rendering stats in the style.
pub(crate) struct RenderingStatsView {
    options: RenderingStatsViewOptions,
    last_update: Option<Instant>,
    frame_count: u32,
    encoding_time: f64,
    rendering_time: f64,
}

/// The id of the view's symbol layer.
pub(crate) const RENDERING_STATS_LAYER_ID: &str = "rendering-stats";
/// The id of the view's source, a point that the layer labels.
pub(crate) const RENDERING_STATS_SOURCE_ID: &str = "rendering-stats-source";

impl RenderingStatsView {
    #[must_use]
    pub(crate) fn new(options: RenderingStatsViewOptions) -> Self {
        RenderingStatsView { options, last_update: None, frame_count: 0, encoding_time: 0.0, rendering_time: 0.0 }
    }

    /// `create(style)`: adds the source and the layer, if the map's style
    /// lacks them.
    pub(crate) fn create(&self, map: &mut Map) {
        if map.source(RENDERING_STATS_SOURCE_ID).is_none() {
            // A GeoJSON source of one point, which the layer labels. The view
            // names the source type as style JSON does, so a registry
            // without it gives no source.
            let json = serde_json::json!({
                "type": "geojson",
                "maxzoom": 0,
                "data": { "type": "Point", "coordinates": [0.0, 0.0] },
            });
            let mut error = mln_style::conversion::Error::default();
            let registry = map.style().registry();
            let source = convert_source(&Convertible::from(&json), &mut error, RENDERING_STATS_SOURCE_ID, registry);
            if let Some(source) = source {
                let _ = map.add_source(source);
            }
        }

        let style = map.style();
        if style.get_layer(RENDERING_STATS_LAYER_ID).is_none()
            // A registry without the symbol layer type has no layer to show.
            && let Ok(mut layer) =
                Layer::new(style.registry(), "symbol", RENDERING_STATS_LAYER_ID, RENDERING_STATS_SOURCE_ID)
        {
            let translation = f64::from(EXTENT as f32 / 2.0 - 100.0);
            let properties = [
                // required
                ("symbol-screen-space", Value::Bool(true)),
                ("text-allow-overlap", Value::Bool(true)),
                ("icon-allow-overlap", Value::Bool(true)),
                ("text-ignore-placement", Value::Bool(true)),
                // customizable
                ("text-color", self.options.text_color.serialize()),
                ("text-size", Value::Double(f64::from(self.options.text_size))),
                ("text-max-width", Value::Double(300.0)),
                ("text-justify", Value::String("left".to_owned())),
                ("text-anchor", Value::String("top-right".to_owned())),
                ("text-translate-anchor", Value::String("viewport".to_owned())),
                ("text-translate", Value::Array(vec![Value::Double(translation), Value::Double(translation)])),
            ];
            for (name, value) in &properties {
                let error = layer.set_property(name, &Convertible::from(value), None);
                debug_assert!(error.is_none(), "{name}: {error:?}");
            }

            let _ = map.apply_loaded(Edit::AddLayer { layer, before: None });
        }
    }

    /// `destroy(style)`: removes the layer and the source.
    #[expect(clippy::unused_self, reason = "the view removes what it added, as its `create` does")]
    pub(crate) fn destroy(&self, map: &mut Map) {
        let _ = map.apply_loaded(Edit::RemoveLayer(RENDERING_STATS_LAYER_ID.to_owned()));
        map.remove_source(RENDERING_STATS_SOURCE_ID);
    }

    /// `update(style, stats)`: counts a frame, and shows the stats of the
    /// frame and the mean times since the last update once the update
    /// interval has passed. Adds the layer back if the style lost it.
    pub(crate) fn update(&mut self, map: &mut Map, stats: &RenderingStats) {
        if map.style().get_layer(RENDERING_STATS_LAYER_ID).is_none() {
            self.create(map);
        }

        self.frame_count += 1;
        self.encoding_time += stats.encoding_time;
        self.rendering_time += stats.rendering_time;

        let now = Instant::now();
        if self.last_update.is_some_and(|last| now.duration_since(last) < self.options.update_interval) {
            return;
        }

        let text = self.text(stats);
        let result = map.apply_loaded(Edit::SetLayerProperty {
            layer: RENDERING_STATS_LAYER_ID.to_owned(),
            name: "text-field".to_owned(),
            value: serde_json::Value::String(text),
        });
        debug_assert!(result.is_ok(), "text-field: {result:?}");

        self.frame_count = 0;
        self.encoding_time = 0.0;
        self.rendering_time = 0.0;
        self.last_update = Some(now);
    }

    /// The text of the stats, one per line.
    fn text(&self, stats: &RenderingStats) -> String {
        let verbose = self.options.verbose;
        let frames = f64::from(self.frame_count.max(1));
        let mut text = String::new();
        let _ = writeln!(text, "Encoding time (ms): {:7.3}", self.encoding_time / frames * 1000.0);
        let _ = writeln!(text, "Rendering time (ms): {:7.3}", self.rendering_time / frames * 1000.0);

        let t = &mut text;
        number(t, "Frame count", f64::from(stats.num_frames), true);
        number(t, "Draw calls", f64::from(stats.num_draw_calls), true);
        number(t, "Total draw calls", f64::from(stats.total_draw_calls), verbose);

        number(t, "Textures", f64::from(stats.num_active_textures), true);
        number(t, "Total textures", f64::from(stats.num_created_textures), verbose);
        number(t, "Texture updates", f64::from(stats.num_texture_updates), verbose);
        memory(t, "Texture updates", stats.texture_update_bytes as u64, verbose);

        number(t, "Buffers", f64::from(stats.num_buffers), true);
        number(t, "Total buffers", stats.total_buffers as f64, verbose);
        number(t, "Total buffer updates", stats.buffer_updates as f64, verbose);
        memory(t, "Total buffer updates", stats.buffer_update_bytes as u64, verbose);

        number(t, "Index buffers", f64::from(stats.num_index_buffers), true);
        memory(t, "Index buffers updates", stats.index_update_bytes as u64, verbose);

        number(t, "Vertex buffers", f64::from(stats.num_vertex_buffers), true);
        memory(t, "Vertex buffers updates", stats.vertex_update_bytes as u64, verbose);

        number(t, "Uniform buffers", f64::from(stats.num_uniform_buffers), true);
        number(t, "Uniform buffer updates", f64::from(stats.num_uniform_updates), verbose);
        memory(t, "Uniform buffer updates", stats.uniform_update_bytes as u64, verbose);

        let bytes = |value: i32| u64::try_from(value).unwrap_or(0);
        memory(t, "Texture memory", bytes(stats.mem_textures), true);
        memory(t, "Buffer memory", bytes(stats.mem_buffers), true);
        memory(t, "Index buffer memory", bytes(stats.mem_index_buffers), true);
        memory(t, "Vertex buffer memory", bytes(stats.mem_vertex_buffers), true);
        memory(t, "Uniform buffer memory", bytes(stats.mem_uniform_buffers), true);

        number(t, "Stencil buffer clears", f64::from(stats.stencil_clears), true);
        number(t, "Stencil buffer updates", f64::from(stats.stencil_updates), verbose);
        text
    }
}

/// Appends a line with a count to `text` if `print` is set.
fn number(text: &mut String, label: &str, value: f64, print: bool) {
    if print {
        let _ = writeln!(text, "{label}: {}", format_number(value));
    }
}

/// Appends a line with a byte count to `text` if `print` is set.
fn memory(text: &mut String, label: &str, value: u64, print: bool) {
    if print {
        let _ = writeln!(text, "{label}: {}", format_memory(value));
    }
}

/// A count, with a K, M or B suffix from a thousand up.
fn format_number(value: f64) -> String {
    if value >= 1e9 {
        format!("{:.3}B", value / 1e9)
    } else if value >= 1e6 {
        format!("{:.3}M", value / 1e6)
    } else if value >= 1e3 {
        format!("{:.3}K", value / 1e3)
    } else {
        format!("{value}")
    }
}

/// A byte count, in whole KB, MB or GB from a kibibyte up.
fn format_memory(value: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if value >= GB {
        format!("{}GB", value / GB)
    } else if value >= MB {
        format!("{}MB", value / MB)
    } else if value >= KB {
        format!("{}KB", value / KB)
    } else {
        format!("{value}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_memory() {
        assert_eq!(format_number(12.0), "12");
        assert_eq!(format_number(1234.0), "1.234K");
        assert_eq!(format_number(2_500_000.0), "2.500M");
        assert_eq!(format_memory(1023), "1023B");
        assert_eq!(format_memory(1536), "1KB");
        assert_eq!(format_memory(3 * 1024 * 1024 * 1024), "3GB");
    }

    #[test]
    fn text_shows_the_mean_times() {
        let mut view = RenderingStatsView::new(RenderingStatsViewOptions::default());
        view.frame_count = 2;
        view.encoding_time = 0.004;
        view.rendering_time = 0.001;
        let stats = RenderingStats { num_draw_calls: 7, mem_textures: 2048, ..Default::default() };
        let text = view.text(&stats);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Encoding time (ms):   2.000");
        assert_eq!(lines[1], "Rendering time (ms):   0.500");
        assert!(lines.contains(&"Draw calls: 7"));
        assert!(lines.contains(&"Texture memory: 2KB"));
        assert!(!text.contains("Total draw calls"));
        assert_eq!(lines.len(), 15);
    }
}
