use std::fmt::Write;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use geojson::{Feature, GeoJSON, Geometry, Point};
use mln_renderer::RenderingStats;
use mln_style::expression::Formatted;
use mln_style::layers::symbol_layer::SymbolLayer;
use mln_style::sources::custom_geometry_source::{
    CustomGeometrySource, CustomGeometrySourceHandle, Options as CustomGeometrySourceOptions,
};
use mln_style::{PropertyValue, Style};
use mln_style_types::{SymbolAnchorType, TextJustifyType, TranslateAnchorType};
use mln_tile_id::CanonicalTileID;
use mln_util::Color;
use mln_util::constants::EXTENT;
use mln_util::range::Range;
use mln_util::time::Instant;

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

    /// `create(style)`: adds the source and the layer, if the style lacks
    /// them.
    pub(crate) fn create(&self, style: &mut Style) {
        if style.get_source(RENDERING_STATS_SOURCE_ID).is_none() {
            let handle: Arc<OnceLock<CustomGeometrySourceHandle>> = Arc::new(OnceLock::new());
            let fetch_handle = Arc::clone(&handle);
            let options = CustomGeometrySourceOptions {
                fetch_tile_function: Some(Arc::new(move |tile_id: &CanonicalTileID| {
                    if let Some(handle) = fetch_handle.get() {
                        let point = Feature::new(Geometry::Point(Point::new(0.0, 0.0)));
                        handle.set_tile_data(*tile_id, GeoJSON::FeatureCollection(vec![point]));
                    }
                })),
                zoom_range: Range::new(0, 0),
                ..Default::default()
            };
            // The default tile options are valid, so this always succeeds.
            if let Ok(source) = CustomGeometrySource::new(RENDERING_STATS_SOURCE_ID, options) {
                let _ = handle.set(source.handle());
                let _ = style.add_source(Box::new(source));
            }
        }

        if style.get_layer(RENDERING_STATS_LAYER_ID).is_none() {
            let mut layer = SymbolLayer::new(RENDERING_STATS_LAYER_ID, RENDERING_STATS_SOURCE_ID);

            // required
            layer.set_symbol_screen_space(&PropertyValue::Constant(true));
            layer.set_text_allow_overlap(&PropertyValue::Constant(true));
            layer.set_icon_allow_overlap(&PropertyValue::Constant(true));
            layer.set_text_ignore_placement(&PropertyValue::Constant(true));

            // customizable
            layer.set_text_color(&PropertyValue::Constant(self.options.text_color));
            layer.set_text_size(&PropertyValue::Constant(self.options.text_size));
            layer.set_text_max_width(&PropertyValue::Constant(300.0));
            layer.set_text_justify(&PropertyValue::Constant(TextJustifyType::Left));
            layer.set_text_anchor(&PropertyValue::Constant(SymbolAnchorType::TopRight));

            let translation = EXTENT as f32 / 2.0 - 100.0;
            layer.set_text_translate_anchor(&PropertyValue::Constant(TranslateAnchorType::Viewport));
            layer.set_text_translate(&PropertyValue::Constant([translation, translation]));

            let _ = style.add_layer(Box::new(layer), None);
        }
    }

    /// `destroy(style)`: removes the layer and the source.
    #[expect(clippy::unused_self, reason = "the view removes what it added, as its `create` does")]
    pub(crate) fn destroy(&self, style: &mut Style) {
        style.remove_layer(RENDERING_STATS_LAYER_ID);
        style.remove_source(RENDERING_STATS_SOURCE_ID);
    }

    /// `update(style, stats)`: counts a frame, and shows the stats of the
    /// frame and the mean times since the last update once the update
    /// interval has passed. Adds the layer back if the style lost it.
    pub(crate) fn update(&mut self, style: &mut Style, stats: &RenderingStats) {
        if style.get_layer(RENDERING_STATS_LAYER_ID).is_none() {
            self.create(style);
        }

        self.frame_count += 1;
        self.encoding_time += stats.encoding_time;
        self.rendering_time += stats.rendering_time;

        let now = Instant::now();
        if self.last_update.is_some_and(|last| now.duration_since(last) < self.options.update_interval) {
            return;
        }

        let text = self.text(stats);
        if let Some(mut layer) = style.get_layer_mut(RENDERING_STATS_LAYER_ID)
            && let Some(layer) = layer.downcast_mut::<SymbolLayer>()
        {
            layer.set_text_field(&PropertyValue::Constant(Formatted::from_plain(&text)));
        }

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
