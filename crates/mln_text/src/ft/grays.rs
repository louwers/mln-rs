// Ports the FT_INT64 code paths of
// ../maplibre-native/vendor/freetype/src/smooth/ftgrays.c (the "smooth"
// anti-aliasing rasterizer of FreeType 2.13) and the overlap rendering of
// src/smooth/ftsmooth.c (`ft_smooth_raster_overlap`,
// `ft_smooth_overlap_spans`): same cells, same fixed-point line walk, same
// conic DDA and cubic bisection, same fill rule.
//
// FreeType renders in bands and bisects a band when its cell pool (16 KiB)
// overflows; the band layout does not change the result, so this port
// renders the whole clip box as one band with an unbounded cell list.

/// `PIXEL_BITS`: 1/256 of a pixel.
const PIXEL_BITS: i32 = 8;
const ONE_PIXEL: i64 = 1 << PIXEL_BITS;

#[inline]
fn trunc(x: i64) -> i32 {
    (x >> PIXEL_BITS) as i32
}
#[inline]
fn fract(x: i64) -> i32 {
    (x & (ONE_PIXEL - 1)) as i32
}
/// `UPSCALE`: 26.6 -> 1/256.
#[inline]
fn upscale(x: i64) -> i64 {
    x * (ONE_PIXEL >> 6)
}

/// A contour command in 26.6, as `FT_Outline_Decompose` emits them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Segment {
    MoveTo(i64, i64),
    LineTo(i64, i64),
    ConicTo(i64, i64, i64, i64),
    CubicTo(i64, i64, i64, i64, i64, i64),
}

/// `TCell` (without the list link: cells of a row are kept sorted).
#[derive(Clone, Copy, Debug)]
struct Cell {
    x: i32,
    cover: i32,
    area: i32,
}

/// `gray_TWorker`.
struct Worker {
    min_ex: i32,
    max_ex: i32,
    min_ey: i32,
    max_ey: i32,
    /// Cells per row (`ycells`), each sorted by x.
    rows: Vec<Vec<Cell>>,
    /// The current cell: (row index, index in row), or None for the
    /// dumpster cell (`cell_null`).
    cell: Option<(usize, usize)>,
    x: i64,
    y: i64,
}

impl Worker {
    /// `FT_INTEGRATE`.
    #[inline]
    fn integrate(&mut self, a: i32, b: i32) {
        if let Some((r, i)) = self.cell {
            let c = &mut self.rows[r][i];
            c.cover = c.cover.wrapping_add(a);
            c.area = c.area.wrapping_add(a.wrapping_mul(b));
        }
    }

    /// `gray_set_cell`.
    fn set_cell(&mut self, ex: i32, ey: i32) {
        let ey_index = ey - self.min_ey;
        if ey_index < 0 || ey_index >= self.max_ey - self.min_ey || ex >= self.max_ex {
            self.cell = None;
            return;
        }
        let ex = ex.max(self.min_ex - 1);
        let row = &mut self.rows[ey_index as usize];
        let i = match row.binary_search_by(|c| c.x.cmp(&ex)) {
            Ok(i) => i,
            Err(i) => {
                row.insert(i, Cell { x: ex, cover: 0, area: 0 });
                i
            }
        };
        self.cell = Some((ey_index as usize, i));
    }

    /// `gray_render_line` (the `FT_INT64` version).
    fn render_line(&mut self, to_x: i64, to_y: i64) {
        let mut ey1 = trunc(self.y);
        let ey2 = trunc(to_y);

        // perform vertical clipping
        if (ey1 >= self.max_ey && ey2 >= self.max_ey) || (ey1 < self.min_ey && ey2 < self.min_ey) {
            self.x = to_x;
            self.y = to_y;
            return;
        }

        let mut ex1 = trunc(self.x);
        let ex2 = trunc(to_x);
        let mut fx1 = fract(self.x);
        let mut fy1 = fract(self.y);

        let dx = to_x - self.x;
        let dy = to_y - self.y;

        if ex1 == ex2 && ey1 == ey2 {
            // inside one cell
        } else if dy == 0 {
            // any horizontal line
            self.set_cell(ex2, ey2);
            self.x = to_x;
            self.y = to_y;
            return;
        } else if dx == 0 {
            if dy > 0 {
                // vertical line up
                loop {
                    let fy2 = ONE_PIXEL as i32;
                    self.integrate(fy2 - fy1, fx1 * 2);
                    fy1 = 0;
                    ey1 += 1;
                    self.set_cell(ex1, ey1);
                    if ey1 == ey2 {
                        break;
                    }
                }
            } else {
                // vertical line down
                loop {
                    let fy2 = 0;
                    self.integrate(fy2 - fy1, fx1 * 2);
                    fy1 = ONE_PIXEL as i32;
                    ey1 -= 1;
                    self.set_cell(ex1, ey1);
                    if ey1 == ey2 {
                        break;
                    }
                }
            }
        } else {
            // any other line
            let mut prod: i64 = dx * i64::from(fy1) - dy * i64::from(fx1);
            // FT_UDIVPREP: (FT_Int64)0xFFFFFFFF / divisor, if needed.
            let dx_r: i64 = if ex1 == ex2 { 0 } else { 0xFFFF_FFFFi64 / dx };
            let dy_r: i64 = if ey1 == ey2 { 0 } else { 0xFFFF_FFFFi64 / dy };
            // FT_UDIV(a, b): ((FT_UInt64)a * (FT_UInt64)b_r) >> 32
            let udiv = |a: i64, b_r: i64| -> i32 { ((a as u64).wrapping_mul(b_r as u64) >> 32) as i32 };

            loop {
                let (fx2, fy2);
                if prod - dx * ONE_PIXEL > 0 && prod <= 0 {
                    // left
                    fx2 = 0;
                    fy2 = udiv(-prod, -dx_r);
                    prod -= dy * ONE_PIXEL;
                    self.integrate(fy2 - fy1, fx1 + fx2);
                    fx1 = ONE_PIXEL as i32;
                    fy1 = fy2;
                    ex1 -= 1;
                } else if prod - dx * ONE_PIXEL + dy * ONE_PIXEL > 0 && prod - dx * ONE_PIXEL <= 0 {
                    // up
                    prod -= dx * ONE_PIXEL;
                    fx2 = udiv(-prod, dy_r);
                    fy2 = ONE_PIXEL as i32;
                    self.integrate(fy2 - fy1, fx1 + fx2);
                    fx1 = fx2;
                    fy1 = 0;
                    ey1 += 1;
                } else if prod + dy * ONE_PIXEL >= 0 && prod - dx * ONE_PIXEL + dy * ONE_PIXEL <= 0 {
                    // right
                    prod += dy * ONE_PIXEL;
                    fx2 = ONE_PIXEL as i32;
                    fy2 = udiv(prod, dx_r);
                    self.integrate(fy2 - fy1, fx1 + fx2);
                    fx1 = 0;
                    fy1 = fy2;
                    ex1 += 1;
                } else {
                    // down
                    fx2 = udiv(prod, -dy_r);
                    fy2 = 0;
                    prod += dx * ONE_PIXEL;
                    self.integrate(fy2 - fy1, fx1 + fx2);
                    fx1 = fx2;
                    fy1 = ONE_PIXEL as i32;
                    ey1 -= 1;
                }
                self.set_cell(ex1, ey1);
                if ex1 == ex2 && ey1 == ey2 {
                    break;
                }
            }
        }

        let fx2 = fract(to_x);
        let fy2 = fract(to_y);
        self.integrate(fy2 - fy1, fx1 + fx2);

        self.x = to_x;
        self.y = to_y;
    }

    /// `gray_render_conic` (the `FT_INT64` DDA version); 26.6 arguments.
    fn render_conic(&mut self, cx: i64, cy: i64, tx: i64, ty: i64) {
        let (p0x, p0y) = (self.x, self.y);
        let (p1x, p1y) = (upscale(cx), upscale(cy));
        let (p2x, p2y) = (upscale(tx), upscale(ty));

        // short-cut the arc that crosses the current band
        if (trunc(p0y) >= self.max_ey && trunc(p1y) >= self.max_ey && trunc(p2y) >= self.max_ey)
            || (trunc(p0y) < self.min_ey && trunc(p1y) < self.min_ey && trunc(p2y) < self.min_ey)
        {
            self.x = p2x;
            self.y = p2y;
            return;
        }

        let bx = p1x - p0x;
        let by = p1y - p0y;
        let ax = p2x - p1x - bx; // p0.x + p2.x - 2 * p1.x
        let ay = p2y - p1y - by;

        let mut dx = ax.abs();
        let dy = ay.abs();
        if dx < dy {
            dx = dy;
        }

        if dx <= ONE_PIXEL / 4 {
            self.render_line(p2x, p2y);
            return;
        }

        // Each bisection reduces the deviation exactly 4-fold.
        let mut shift = 16;
        loop {
            dx >>= 2;
            shift -= 1;
            if dx <= ONE_PIXEL / 4 {
                break;
            }
        }
        let mut count: u32 = 0x10000u32 >> shift;

        let left_shift = |a: i64, b: i32| -> i64 { ((a as u64) << b) as i64 };
        let mut rx = left_shift(ax, shift + shift);
        let mut ry = left_shift(ay, shift + shift);
        let mut qx = left_shift(bx, shift + 17).wrapping_add(rx);
        let mut qy = left_shift(by, shift + 17).wrapping_add(ry);
        rx = rx.wrapping_mul(2);
        ry = ry.wrapping_mul(2);
        let mut px = left_shift(p0x, 32);
        let mut py = left_shift(p0y, 32);

        loop {
            px = px.wrapping_add(qx);
            py = py.wrapping_add(qy);
            qx = qx.wrapping_add(rx);
            qy = qy.wrapping_add(ry);
            self.render_line(px >> 32, py >> 32);
            count -= 1;
            if count == 0 {
                break;
            }
        }
    }

    /// `gray_render_cubic` with `gray_split_cubic`; 26.6 arguments.
    fn render_cubic(&mut self, c1x: i64, c1y: i64, c2x: i64, c2y: i64, tx: i64, ty: i64) {
        // bez_stack: enough to accommodate bisections
        let mut stack = [(0i64, 0i64); 16 * 3 + 1];
        let mut arc = 0usize;
        stack[0] = (upscale(tx), upscale(ty));
        stack[1] = (upscale(c2x), upscale(c2y));
        stack[2] = (upscale(c1x), upscale(c1y));
        stack[3] = (self.x, self.y);

        // short-cut the arc that crosses the current band
        let above = stack[..4].iter().all(|p| trunc(p.1) >= self.max_ey);
        let below = stack[..4].iter().all(|p| trunc(p.1) < self.min_ey);
        if above || below {
            self.x = stack[0].0;
            self.y = stack[0].1;
            return;
        }

        loop {
            let a = &stack[arc..arc + 4];
            let half = ONE_PIXEL / 2;
            let split = (2 * a[0].0 - 3 * a[1].0 + a[3].0).abs() > half
                || (2 * a[0].1 - 3 * a[1].1 + a[3].1).abs() > half
                || (a[0].0 - 3 * a[2].0 + 2 * a[3].0).abs() > half
                || (a[0].1 - 3 * a[2].1 + 2 * a[3].1).abs() > half;
            if split {
                split_cubic(&mut stack[arc..arc + 7]);
                arc += 3;
                continue;
            }
            let (x, y) = stack[arc];
            self.render_line(x, y);
            if arc == 0 {
                return;
            }
            arc -= 3;
        }
    }

    /// `gray_move_to`.
    fn move_to(&mut self, x: i64, y: i64) {
        let (x, y) = (upscale(x), upscale(y));
        self.set_cell(trunc(x), trunc(y));
        self.x = x;
        self.y = y;
    }

    /// The rows (bottom to top) as (cells, `min_ex`) for the sweeps.
    fn decompose(&mut self, outline: &[Segment]) {
        // FT_Outline_Decompose closes every contour with a line to its
        // start; the segments here already contain those lines.
        for s in outline {
            match *s {
                Segment::MoveTo(x, y) => self.move_to(x, y),
                Segment::LineTo(x, y) => self.render_line(upscale(x), upscale(y)),
                Segment::ConicTo(cx, cy, x, y) => self.render_conic(cx, cy, x, y),
                Segment::CubicTo(c1x, c1y, c2x, c2y, x, y) => self.render_cubic(c1x, c1y, c2x, c2y, x, y),
            }
        }
    }
}

/// `gray_split_cubic` on base[0..7].
fn split_cubic(base: &mut [(i64, i64)]) {
    for k in 0..2 {
        let get = |b: &[(i64, i64)], i: usize| if k == 0 { b[i].0 } else { b[i].1 };
        let set = |b: &mut [(i64, i64)], i: usize, v: i64| {
            if k == 0 { b[i].0 = v } else { b[i].1 = v }
        };
        let b3 = get(base, 3);
        set(base, 6, b3);
        let mut a = get(base, 0) + get(base, 1);
        let b = get(base, 1) + get(base, 2);
        let mut c = get(base, 2) + get(base, 3);
        set(base, 5, c >> 1);
        c += b;
        set(base, 4, c >> 2);
        set(base, 1, a >> 1);
        a += b;
        set(base, 2, a >> 2);
        set(base, 3, (a + c) >> 3);
    }
}

/// `FT_FILL_RULE` with the non-zero rule (`fill = INT_MIN`; TrueType and
/// CFF outlines never set `FT_OUTLINE_EVEN_ODD_FILL`).
#[inline]
fn fill_rule(area: i32) -> i32 {
    let fill = i32::MIN;
    let mut coverage = area >> (PIXEL_BITS * 2 + 1 - 8);
    if coverage & fill != 0 {
        coverage = !coverage;
    }
    if coverage > 255 && fill & i32::MIN != 0 {
        coverage = 255;
    }
    coverage
}

fn new_worker(min_ex: i32, min_ey: i32, max_ex: i32, max_ey: i32) -> Worker {
    Worker {
        min_ex,
        max_ex,
        min_ey,
        max_ey,
        rows: vec![Vec::new(); (max_ey - min_ey).max(0) as usize],
        cell: None,
        x: 0,
        y: 0,
    }
}

/// `gray_sweep` / `gray_sweep_direct`: calls `span(y, x, len, coverage)`.
fn sweep(w: &Worker, mut span: impl FnMut(i32, i32, i32, u8)) {
    for y in w.min_ey..w.max_ey {
        let mut x = w.min_ex;
        let mut cover: i32 = 0;
        for cell in &w.rows[(y - w.min_ey) as usize] {
            if cover != 0 && cell.x > x {
                span(y, x, cell.x - x, fill_rule(cover) as u8);
            }
            cover = cover.wrapping_add(cell.cover.wrapping_mul((ONE_PIXEL * 2) as i32));
            let area = cover.wrapping_sub(cell.area);
            if area != 0 && cell.x >= w.min_ex {
                span(y, cell.x, 1, fill_rule(area) as u8);
            }
            x = cell.x + 1;
        }
        if cover != 0 {
            // only if cropped
            span(y, x, w.max_ex - x, fill_rule(cover) as u8);
        }
    }
}

/// `gray_raster_render` into a gray bitmap of `width` x `rows`, top row
/// first (FreeType's positive pitch), for an outline already translated
/// into the bitmap (26.6, y up).
#[must_use]
pub fn render(outline: &[Segment], width: u32, rows: u32, overlap: bool) -> Vec<u8> {
    let mut bitmap = vec![0u8; (width * rows) as usize];
    if outline.is_empty() || width == 0 || rows == 0 {
        return bitmap;
    }
    let pitch = width as i32;
    if overlap {
        // ft_smooth_raster_overlap: render at SCALE = 4 in both directions
        // and average the spans (ft_smooth_overlap_spans).
        const SCALE: i64 = 1 << 2;
        if i64::from(width) * SCALE > 0x7FFF {
            return bitmap; // Raster_Overflow: the bitmap stays empty
        }
        let inflated: Vec<Segment> = outline
            .iter()
            .map(|s| match *s {
                Segment::MoveTo(x, y) => Segment::MoveTo(x * SCALE, y * SCALE),
                Segment::LineTo(x, y) => Segment::LineTo(x * SCALE, y * SCALE),
                Segment::ConicTo(a, b, x, y) => Segment::ConicTo(a * SCALE, b * SCALE, x * SCALE, y * SCALE),
                Segment::CubicTo(a, b, c, d, x, y) => {
                    Segment::CubicTo(a * SCALE, b * SCALE, c * SCALE, d * SCALE, x * SCALE, y * SCALE)
                }
            })
            .collect();
        let mut w = new_worker(0, 0, (i64::from(width) * SCALE) as i32, (i64::from(rows) * SCALE) as i32);
        w.decompose(&inflated);
        sweep(&w, |y, x, len, coverage| {
            let row = (rows as i32 - 1 - y / SCALE as i32) * pitch;
            let cover = (u32::from(coverage) + (SCALE * SCALE / 2) as u32) / (SCALE * SCALE) as u32;
            // FT_Span.x and len are 16-bit.
            let (x, len) = (i32::from(x as i16), i32::from(len as u16));
            for i in 0..len {
                let idx = (row + (x + i) / SCALE as i32) as usize;
                let sum = u32::from(bitmap[idx]) + cover;
                bitmap[idx] = (sum - (sum >> 8)) as u8;
            }
        });
    } else {
        let mut w = new_worker(0, 0, width as i32, rows as i32);
        w.decompose(outline);
        // line = origin - pitch * y, origin at the bottom row.
        sweep(&w, |y, x, len, coverage| {
            let row = (rows as i32 - 1 - y) * pitch;
            for i in 0..len {
                bitmap[(row + x + i) as usize] = coverage;
            }
        });
    }
    bitmap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pixel_square() {
        // A 2x2 px square at (1,1): coverage 255 inside.
        let o = [
            Segment::MoveTo(64, 64),
            Segment::LineTo(64, 192),
            Segment::LineTo(192, 192),
            Segment::LineTo(192, 64),
            Segment::LineTo(64, 64),
        ];
        let b = render(&o, 4, 4, false);
        assert_eq!(b, vec![0, 0, 0, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn half_pixel_coverage() {
        let o = [
            Segment::MoveTo(0, 0),
            Segment::LineTo(0, 64),
            Segment::LineTo(32, 64),
            Segment::LineTo(32, 0),
            Segment::LineTo(0, 0),
        ];
        assert_eq!(render(&o, 1, 1, false), vec![128]);
    }
}
