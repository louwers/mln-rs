// C++ keeps the text as line-list vertices for the DebugShader. WebGPU draws
// line primitives one pixel wide, so the text is kept as strokes instead,
// which TileSourceRenderItem draws as polylines, like the tile borders.

use geometry::Point;
use mln_map_types::MapDebugOptions;
use mln_tile_id::OverscaledTileID;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::chrono::{Timestamp, iso8601};

use crate::geometry::debug_font_data::SIMPLEX;

/// The tile's parse status and timestamps, as text in tile coordinates.
pub(crate) struct DebugBucket {
    pub(crate) renderable: bool,
    pub(crate) complete: bool,
    pub(crate) modified: Option<Timestamp>,
    pub(crate) expires: Option<Timestamp>,
    pub(crate) debug_mode: MapDebugOptions,

    /// The text, one line per entry, empty without `PARSE_STATUS` and
    /// `TIMESTAMPS`.
    pub(crate) text: Vec<String>,
    /// The strokes of the text's glyphs.
    pub(crate) strokes: Vec<GeometryCoordinates>,
}

impl DebugBucket {
    #[must_use]
    pub(crate) fn new(
        id: &OverscaledTileID,
        renderable: bool,
        complete: bool,
        modified: Option<Timestamp>,
        expires: Option<Timestamp>,
        debug_mode: MapDebugOptions,
        source_name: &str,
    ) -> Self {
        let mut text = Vec::new();
        let mut strokes = Vec::new();
        let mut add_text = |line: String, left: f64, baseline: f64, scale: f64| {
            add_strokes(&mut strokes, &line, left, baseline, scale);
            text.push(line);
        };

        let mut baseline = 200.0;
        if debug_mode.contains(MapDebugOptions::PARSE_STATUS) {
            let status = if complete {
                "complete"
            } else if renderable {
                "renderable"
            } else {
                "pending"
            };
            add_text(format!("{id} - {status} {source_name}"), 50.0, baseline, 5.0);
            baseline += 200.0;
        }

        if debug_mode.contains(MapDebugOptions::TIMESTAMPS)
            && let (Some(modified), Some(expires)) = (modified, expires)
        {
            add_text(format!("modified: {}", iso8601(modified)), 50.0, baseline, 5.0);
            add_text(format!("expires: {}", iso8601(expires)), 50.0, baseline + 200.0, 5.0);
        }

        DebugBucket { renderable, complete, modified, expires, debug_mode, text, strokes }
    }
}

/// Appends the strokes of `text`'s printable ASCII characters, starting at
/// `left` on `baseline` (tile units, y down), with font units scaled by
/// `scale`.
fn add_strokes(strokes: &mut Vec<GeometryCoordinates>, text: &str, mut left: f64, baseline: f64, scale: f64) {
    for c in text.bytes() {
        if !(32..127).contains(&c) {
            continue;
        }
        let glyph = &SIMPLEX[usize::from(c - 32)];
        let mut stroke = GeometryCoordinates::new();
        for pair in glyph.data.as_chunks::<2>().0 {
            if *pair == [-1, -1] {
                finish_stroke(strokes, &mut stroke);
            } else {
                stroke.push(Point::new(
                    (left + f64::from(pair[0]) * scale).round() as i16,
                    (baseline - f64::from(pair[1]) * scale).round() as i16,
                ));
            }
        }
        finish_stroke(strokes, &mut stroke);
        left += f64::from(glyph.width) * scale;
    }
}

/// Keeps a stroke of two or more points; a single point draws nothing.
fn finish_stroke(strokes: &mut Vec<GeometryCoordinates>, stroke: &mut GeometryCoordinates) {
    if stroke.len() >= 2 {
        strokes.push(std::mem::take(stroke));
    } else {
        stroke.clear();
    }
}

#[cfg(test)]
mod tests {
    use mln_tile_id::CanonicalTileID;

    use super::*;

    fn bucket(debug_mode: MapDebugOptions, modified: Option<Timestamp>, expires: Option<Timestamp>) -> DebugBucket {
        let id = OverscaledTileID::new(3, 0, CanonicalTileID::new(2, 1, 3));
        DebugBucket::new(&id, true, false, modified, expires, debug_mode, "streets")
    }

    #[test]
    fn parse_status_text() {
        let bucket = bucket(MapDebugOptions::PARSE_STATUS, None, None);
        assert_eq!(bucket.text, ["2/1/3=>3 - renderable streets"]);
        assert_ne!(bucket.strokes, Vec::<GeometryCoordinates>::new());
    }

    #[test]
    fn timestamps_need_both_times() {
        let time = Some(Timestamp::from_secs(784_111_777));
        assert_eq!(bucket(MapDebugOptions::TIMESTAMPS, time, None).text, Vec::<String>::new());
        let bucket = bucket(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS, time, time);
        assert_eq!(
            bucket.text,
            ["2/1/3=>3 - renderable streets", "modified: 1994-11-06 08:49:37", "expires: 1994-11-06 08:49:37",]
        );
    }

    #[test]
    fn tile_borders_have_no_text() {
        let bucket = bucket(MapDebugOptions::TILE_BORDERS, None, None);
        assert_eq!(bucket.text, Vec::<String>::new());
        assert_eq!(bucket.strokes, Vec::<GeometryCoordinates>::new());
    }

    #[test]
    fn strokes_follow_the_font() {
        // '1' is one stroke of four points: (6, 17), (8, 18), (11, 21), (11, 0).
        let mut strokes = Vec::new();
        add_strokes(&mut strokes, "1", 50.0, 200.0, 5.0);
        assert_eq!(
            strokes,
            [vec![Point::new(80, 115), Point::new(90, 110), Point::new(105, 95), Point::new(105, 200)]]
        );

        // '!' is two strokes, and a space or a non-ASCII character only
        // advances or is skipped.
        let mut strokes = Vec::new();
        add_strokes(&mut strokes, " é!", 0.0, 0.0, 1.0);
        assert_eq!(strokes.len(), 2);
        assert_eq!(strokes[0][0], Point::new(16 + 5, -21));
    }
}
