//! A font face's glyph bitmaps: the glyph's hinted outline at
//! `SDF_FONT_SIZE` from skrifa, filled by `ab_glyph_rasterizer`.
//!
//! The bitmap covers the outline's control box, points and control points
//! alike, rounded out to whole pixels. A glyph whose outline flags
//! overlapping contours is drawn at four times the size in both directions
//! and averaged down, since adding up the contours' coverage would darken
//! the pixels on the edges where they overlap.
//!
//! A glyph without an outline, such as a color emoji font's bitmap glyph,
//! gives an empty glyph.

use ab_glyph_rasterizer::{Point, Rasterizer, point};
use mln_util::image::AlphaImage;
use mln_util::size::Size;
use skrifa::{DrawMemory, GlyphId, PathSink};

use crate::face::{Face, SDF_FONT_SIZE};
use crate::glyph::{Glyph, GlyphID};

/// One piece of an outline, in pixels with y up.
#[derive(Clone, Copy)]
enum Segment {
    Line(Point, Point),
    Quad(Point, Point, Point),
    Cubic(Point, Point, Point, Point),
}

/// The closed contours of a drawn outline and their control box.
#[derive(Default)]
struct Outline {
    segments: Vec<Segment>,
    start: Option<Point>,
    current: Point,
    /// The control box: `[x_min, y_min, x_max, y_max]`.
    bounds: Option<[f32; 4]>,
}

impl Outline {
    fn add_point(&mut self, x: f32, y: f32) -> Point {
        self.bounds = Some(match self.bounds {
            Some([x_min, y_min, x_max, y_max]) => [x_min.min(x), y_min.min(y), x_max.max(x), y_max.max(y)],
            None => [x, y, x, y],
        });
        point(x, y)
    }

    fn close_contour(&mut self) {
        if let Some(start) = self.start.take()
            && start != self.current
        {
            self.segments.push(Segment::Line(self.current, start));
        }
    }
}

impl PathSink for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close_contour();
        self.current = self.add_point(x, y);
        self.start = Some(self.current);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let to = self.add_point(x, y);
        self.segments.push(Segment::Line(self.current, to));
        self.current = to;
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let c = self.add_point(cx0, cy0);
        let to = self.add_point(x, y);
        self.segments.push(Segment::Quad(self.current, c, to));
        self.current = to;
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let c0 = self.add_point(cx0, cy0);
        let c1 = self.add_point(cx1, cy1);
        let to = self.add_point(x, y);
        self.segments.push(Segment::Cubic(self.current, c0, c1, to));
        self.current = to;
    }
    fn close(&mut self) {
        self.close_contour();
    }
}

fn times(p: Point, k: f32) -> Point {
    point(p.x * k, p.y * k)
}

/// Adds a quadratic Bézier curve to `rasterizer` as lines: 2^k pieces of
/// equal parameter steps, the fewest that keep the curve's second difference
/// within a quarter pixel on each axis (a sixteenth of a pixel off the
/// curve at most).
fn draw_quad(rasterizer: &mut Rasterizer, p0: Point, p1: Point, p2: Point) {
    let a = p0 - times(p1, 2.0) + p2;
    let mut deviation = a.x.abs().max(a.y.abs());
    let mut pieces = 1u32;
    while deviation > 0.25 && pieces < 1 << 16 {
        deviation /= 4.0;
        pieces *= 2;
    }
    let mut from = p0;
    for i in 1..=pieces {
        let t = i as f32 / pieces as f32;
        let u = 1.0 - t;
        let to = if i == pieces { p2 } else { times(p0, u * u) + times(p1, 2.0 * u * t) + times(p2, t * t) };
        rasterizer.draw_line(from, to);
        from = to;
    }
}

/// Adds a cubic Bézier curve to `rasterizer` as lines, halving it until
/// both control points lie within half a pixel, on each axis, of where a
/// straight line would put them.
fn draw_cubic(rasterizer: &mut Rasterizer, p0: Point, p1: Point, p2: Point, p3: Point, depth: u32) {
    let off = |v: Point| v.x.abs() > 0.5 || v.y.abs() > 0.5;
    if depth < 16 && (off(times(p3, 2.0) - times(p2, 3.0) + p0) || off(p3 - times(p1, 3.0) + times(p0, 2.0))) {
        let half = |a: Point, b: Point| times(a + b, 0.5);
        let (p01, p12, p23) = (half(p0, p1), half(p1, p2), half(p2, p3));
        let (p012, p123) = (half(p01, p12), half(p12, p23));
        let mid = half(p012, p123);
        draw_cubic(rasterizer, p0, p01, p012, mid, depth + 1);
        draw_cubic(rasterizer, mid, p123, p23, p3, depth + 1);
    } else {
        rasterizer.draw_line(p0, p3);
    }
}

/// The coverage of `segments` in a bitmap of `width` x `rows` whose top left
/// corner is at (`left`, `top`), top row first. With `oversample`, each
/// pixel averages a 4 x 4 grid of samples.
///
/// The curves' flattening and the coverage values follow FreeType's smooth
/// rasterizer, which drew the render tests' expected images: a pixel's
/// coverage in 256ths, rounded down and kept to 255, and an oversampled
/// pixel's average rounded to nearest.
fn render(segments: &[Segment], left: f32, top: f32, width: usize, rows: usize, oversample: bool) -> Vec<u8> {
    let scale = if oversample { 4 } else { 1 };
    let to_bitmap = |p: Point| point((p.x - left) * scale as f32, (top - p.y) * scale as f32);
    let mut rasterizer = Rasterizer::new(width * scale, rows * scale);
    for segment in segments {
        match *segment {
            Segment::Line(a, b) => rasterizer.draw_line(to_bitmap(a), to_bitmap(b)),
            Segment::Quad(a, b, c) => draw_quad(&mut rasterizer, to_bitmap(a), to_bitmap(b), to_bitmap(c)),
            Segment::Cubic(a, b, c, d) => {
                draw_cubic(&mut rasterizer, to_bitmap(a), to_bitmap(b), to_bitmap(c), to_bitmap(d), 0);
            }
        }
    }
    let mut coverage = vec![0f32; width * rows];
    rasterizer.for_each_pixel_2d(|x, y, alpha| {
        coverage[y as usize / scale * width + x as usize / scale] += alpha.min(1.0);
    });
    let samples = (scale * scale) as f32;
    // The float sums can fall a little short of a whole 256th.
    let rounding = if oversample { 0.5 } else { 1.0 / 64.0 };
    coverage.into_iter().map(|c| (c / samples * 256.0 + rounding).min(255.0) as u8).collect()
}

pub(crate) fn rasterize_glyph(face: &Face, glyph: GlyphID) -> Glyph {
    let border = u32::from(Glyph::BORDER_SIZE);
    let mut fixed_metrics = Glyph { id: glyph, ..Glyph::default() };

    let mut outline = Outline::default();
    let mut advance = 0.0;
    let mut overlap = false;
    if let Some(scaled) = face.scaler().and_then(|s| s.outline(GlyphId::new(u32::from(glyph.code)))) {
        if let Ok(metrics) = scaled.draw(DrawMemory::Scratch, &mut outline) {
            outline.close_contour();
            advance = metrics.h_advance.unwrap_or(0.0).round();
            overlap = metrics.has_overlaps;
        } else {
            outline = Outline::default();
        }
    }

    // The bitmap box: the control box rounded out to whole pixels. A glyph
    // too large for a 16-bit bitmap is empty.
    let [x_min, y_min, x_max, y_max] = outline.bounds.unwrap_or_default().map(f64::from);
    let (left, bottom, right, top) = (x_min.floor(), y_min.floor(), x_max.ceil(), y_max.ceil());
    let fits = [left, bottom, right, top].iter().all(|v| (-32768.0..=32767.0).contains(v));
    let (left, top, width, rows) =
        if fits { (left as i32, top as i32, (right - left) as u32, (top - bottom) as u32) } else { (0, 0, 0, 0) };

    let buffer = if outline.segments.is_empty() || width == 0 || rows == 0 {
        vec![0u8; (width * rows) as usize]
    } else {
        render(&outline.segments, left as f32, top as f32, width as usize, rows as usize, overlap)
    };

    let size = Size::new(width + border * 2, rows + border * 2);
    fixed_metrics.metrics.width = size.width;
    fixed_metrics.metrics.height = size.height;
    fixed_metrics.metrics.left = left as f32;
    fixed_metrics.metrics.top = (top - SDF_FONT_SIZE - border as i32) as f32;
    fixed_metrics.metrics.advance = advance as u32;

    let mut bitmap = AlphaImage::new(size);
    for h in 0..rows {
        let dst = (size.width * (h + border) + border) as usize;
        let src = (width * h) as usize;
        bitmap.data[dst..dst + width as usize].copy_from_slice(&buffer[src..src + width as usize]);
    }
    fixed_metrics.bitmap = bitmap;
    fixed_metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Segment> {
        let (a, b, c, d) = (point(x0, y0), point(x0, y1), point(x1, y1), point(x1, y0));
        vec![Segment::Line(a, b), Segment::Line(b, c), Segment::Line(c, d), Segment::Line(d, a)]
    }

    #[test]
    fn full_pixels() {
        let bitmap = render(&square(1.0, 1.0, 3.0, 3.0), 0.0, 4.0, 4, 4, false);
        assert_eq!(bitmap, [0, 0, 0, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn half_a_pixel() {
        assert_eq!(render(&square(0.0, 0.0, 0.5, 1.0), 0.0, 1.0, 1, 1, false), [128]);
    }

    /// The area a bitmap covers, in pixels.
    fn area(bitmap: &[u8]) -> f32 {
        bitmap.iter().map(|&c| f32::from(c)).sum::<f32>() / 256.0
    }

    /// Curves, flattened into lines inside them, cover all but a few
    /// percent of their area.
    #[test]
    fn curves_cover_their_area() {
        let (a, b) = (point(0.0, 0.0), point(4.0, 0.0));
        // A parabolic segment 4 px wide and 2 px high: 16/3 px².
        let quad = [Segment::Quad(a, point(2.0, 4.0), b), Segment::Line(b, a)];
        let covered = area(&render(&quad, 0.0, 2.0, 4, 2, false)) / (16.0 / 3.0);
        assert!((0.97..=1.0).contains(&covered), "{covered}");
        // Under the cubic (0,0) (0,4) (4,4) (4,0): 9.6 px².
        let cubic = [Segment::Cubic(a, point(0.0, 4.0), point(4.0, 4.0), b), Segment::Line(b, a)];
        let covered = area(&render(&cubic, 0.0, 3.0, 4, 3, false)) / 9.6;
        assert!((0.96..=1.0).contains(&covered), "{covered}");
    }

    #[test]
    fn oversampling_counts_overlapping_contours_once() {
        // Two halves of one pixel, overlapping on its second quarter: three
        // quarters of it covered.
        let mut segments = square(0.0, 0.0, 0.5, 1.0);
        segments.extend(square(0.25, 0.0, 0.75, 1.0));
        assert_eq!(render(&segments, 0.0, 1.0, 1, 1, true), [192]);
        assert_eq!(render(&segments, 0.0, 1.0, 1, 1, false), [255]);
    }
}
