// C++ port of https://github.com/mapbox/tiny-sdf, which is in turn based on
// the Felzenszwalb/Huttenlocher distance transform paper
// (https://cs.brown.edu/~pff/papers/dt-final.pdf).

use crate::image::AlphaImage;

// The distances are `f32`, where C++'s are `double`: that is faster, and
// rounds a rare pixel one level away from C++'s.
const INF: f32 = 1e20;

/// One parabola of the lower envelope: rooted at `root` with the height
/// `value`, and lowest from `start` until the next parabola's `start`.
#[derive(Clone, Copy)]
struct Parabola {
    root: f32,
    value: f32,
    start: f32,
}

/// 1D squared distance transform of `line`, in place. `envelope` is scratch.
fn edt1d(line: &mut [f32], envelope: &mut Vec<Parabola>) {
    let Some((&first, rest)) = line.split_first() else {
        return;
    };
    // A uniform line, such as a column of a glyph's padding, transforms to
    // itself.
    if rest.iter().all(|&value| value == first) {
        return;
    }

    // The lower envelope of the parabolas. A parabola at INF is never the
    // lowest while one isn't, so the envelope leaves them out; a line of
    // nothing but INF is uniform.
    envelope.clear();
    for (q, &value) in line.iter().enumerate() {
        if value >= INF {
            continue;
        }
        let q = q as f32;
        let height = value + q * q;
        let start = loop {
            let Some(top) = envelope.last() else { break -INF };
            let start = (height - (top.value + top.root * top.root)) / (2.0 * q - 2.0 * top.root);
            if start > top.start {
                break start;
            }
            envelope.pop();
        };
        envelope.push(Parabola { root: q, value, start });
    }

    let mut parabolas = envelope.iter();
    let Some(mut current) = parabolas.next() else {
        return;
    };
    let mut next = parabolas.next();
    for (q, out) in line.iter_mut().enumerate() {
        let q = q as f32;
        while let Some(parabola) = next.filter(|parabola| parabola.start < q) {
            current = parabola;
            next = parabolas.next();
        }
        let distance = q - current.root;
        *out = distance * distance + current.value;
    }
}

/// 2D Euclidean distance transform of `data`, `width` pixels wide, in place:
/// squared along the columns, then the rows' square roots.
fn edt(data: &mut [f32], width: usize, column: &mut Vec<f32>, envelope: &mut Vec<Parabola>) {
    for x in 0..width {
        column.clear();
        column.extend(data[x..].iter().step_by(width));
        edt1d(column, envelope);
        for (pixel, &value) in data[x..].iter_mut().step_by(width).zip(column.iter()) {
            *pixel = value;
        }
    }
    for row in data.chunks_exact_mut(width) {
        edt1d(row, envelope);
        for pixel in row {
            *pixel = pixel.sqrt();
        }
    }
}

/// Turns an alpha raster into a signed distance field of the same size.
#[must_use]
pub fn transform_raster_to_sdf(raster_input: &AlphaImage, radius: f64, cutoff: f64) -> AlphaImage {
    let mut sdf = AlphaImage::new(raster_input.size);
    let width = raster_input.size.width as usize;
    let size = width * raster_input.size.height as usize;
    if size == 0 {
        return sdf;
    }

    // `grid_outer` measures the distance to the glyph, and `grid_inner` the
    // distance to its outside: each starts at 0 where a pixel is what it
    // measures the distance to, at INF where it isn't, and in between where
    // the pixel is partly covered.
    let mut grid_outer = Vec::with_capacity(size);
    let mut grid_inner = Vec::with_capacity(size);
    for &alpha in &raster_input.data[..size] {
        let (outer, inner) = match alpha {
            255 => (0.0, INF),
            0 => (INF, 0.0),
            _ => {
                let a = f64::from(alpha) / 255.0;
                ((0.5 - a).max(0.0).powi(2) as f32, (a - 0.5).max(0.0).powi(2) as f32)
            }
        };
        grid_outer.push(outer);
        grid_inner.push(inner);
    }

    let max_dimension = width.max(raster_input.size.height as usize);
    let mut column = Vec::with_capacity(max_dimension);
    let mut envelope = Vec::with_capacity(max_dimension);
    edt(&mut grid_outer, width, &mut column, &mut envelope);
    edt(&mut grid_inner, width, &mut column, &mut envelope);

    for ((pixel, &outer), &inner) in sdf.data.iter_mut().zip(&grid_outer).zip(&grid_inner) {
        let distance = f64::from(outer - inner);
        *pixel = round_to_alpha(255.0 - 255.0 * (distance / radius + cutoff));
    }

    sdf
}

/// Rounds to the nearest integer, halfway cases away from zero, clamped to
/// 0-255: C++'s `std::max(0l, std::min(255l, ::lround(x)))`, without a call
/// to `lround`.
fn round_to_alpha(x: f64) -> u8 {
    if x.is_nan() || x <= 0.0 {
        0
    } else if x >= 255.0 {
        255
    } else {
        let whole = x as u8;
        // The subtraction is exact.
        if x - f64::from(whole) >= 0.5 { whole + 1 } else { whole }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::size::Size;

    #[test]
    fn solid_and_empty() {
        let img = AlphaImage::new(Size::new(3, 3));
        let sdf = transform_raster_to_sdf(&img, 8.0, 0.25);
        assert!(sdf.data.iter().all(|&v| v == 0));
        let mut img = AlphaImage::new(Size::new(1, 1));
        img.data[0] = 255;
        let sdf = transform_raster_to_sdf(&img, 8.0, 0.25);
        assert_eq!(sdf.data, vec![255]);
    }

    /// The distance field by definition: each pixel's squared distance is
    /// the least, over every pixel, of the squared distance to it plus its
    /// own value.
    fn brute_force(raster: &AlphaImage, radius: f64, cutoff: f64) -> Vec<u8> {
        let (width, height) = (raster.size.width as usize, raster.size.height as usize);
        let grid = |inside: bool| -> Vec<f64> {
            raster
                .data
                .iter()
                .map(|&alpha| {
                    let a = f64::from(alpha) / 255.0;
                    match (alpha, inside) {
                        (255, false) | (0, true) => 0.0,
                        (0, false) | (255, true) => f64::INFINITY,
                        (_, false) => (0.5 - a).max(0.0).powi(2),
                        (_, true) => (a - 0.5).max(0.0).powi(2),
                    }
                })
                .collect()
        };
        let distance = |grid: &[f64], x: usize, y: usize| {
            let mut least = f64::INFINITY;
            for (i, &value) in grid.iter().enumerate() {
                let (dx, dy) = ((i % width) as f64 - x as f64, (i / width) as f64 - y as f64);
                least = least.min(dx * dx + dy * dy + value);
            }
            least.sqrt()
        };
        let (outer, inner) = (grid(false), grid(true));
        (0..width * height)
            .map(|i| {
                let (x, y) = (i % width, i / width);
                let d = distance(&outer, x, y).min(1e10) - distance(&inner, x, y).min(1e10);
                (255.0 - 255.0 * (d / radius + cutoff)).round().clamp(0.0, 255.0) as u8
            })
            .collect()
    }

    #[test]
    fn matches_the_distance_field_by_definition() {
        // A fixed pseudo-random sequence: mostly empty and solid pixels,
        // some partly covered.
        let mut state = 0x2545_f491_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for (width, height) in [(1, 7), (7, 1), (5, 5), (12, 9), (30, 30)] {
            for _ in 0..20 {
                let mut raster = AlphaImage::new(Size::new(width, height));
                for pixel in &mut raster.data {
                    *pixel = match next() % 8 {
                        0..=3 => 0,
                        4 | 5 => 255,
                        _ => (next() % 256) as u8,
                    };
                }
                for radius in [8.0, 16.0] {
                    assert_eq!(
                        transform_raster_to_sdf(&raster, radius, 0.25).data,
                        brute_force(&raster, radius, 0.25),
                        "{width}x{height}: {:?}",
                        raster.data
                    );
                }
            }
        }
    }
}
