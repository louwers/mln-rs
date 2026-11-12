// Ports src/mln/util/tiny_sdf.hpp and src/mln/util/tiny_sdf.cpp.
//
// C++ port of https://github.com/mapbox/tiny-sdf, which is in turn based on
// the Felzenszwalb/Huttenlocher distance transform paper
// (https://cs.brown.edu/~pff/papers/dt-final.pdf).

use crate::image::AlphaImage;

const INF: f64 = 1e20;

/// `tinysdf::edt1d`: 1D squared distance transform. `v` is `int16_t` in
/// C++ (indices wrap for dimensions above 32767, as there).
fn edt1d(f: &[f64], d: &mut [f64], v: &mut [i16], z: &mut [f64], n: u32) {
    v[0] = 0;
    z[0] = -INF;
    z[1] = INF;

    let mut k: usize = 0;
    for q in 1..n {
        let qf = f64::from(q);
        let vk = |v: &[i16], k: usize| f64::from(v[k]);
        let mut s =
            ((f[q as usize] + qf * qf) - (f[v[k] as usize] + vk(v, k) * vk(v, k))) / (2.0 * qf - 2.0 * vk(v, k));
        while s <= z[k] {
            k = k.wrapping_sub(1);
            s = ((f[q as usize] + qf * qf) - (f[v[k] as usize] + vk(v, k) * vk(v, k))) / (2.0 * qf - 2.0 * vk(v, k));
        }
        k += 1;
        v[k] = q as i16;
        z[k] = s;
        z[k + 1] = INF;
    }

    let mut k: usize = 0;
    for q in 0..n {
        let qf = f64::from(q);
        while z[k + 1] < qf {
            k += 1;
        }
        let vk = f64::from(v[k]);
        d[q as usize] = (qf - vk) * (qf - vk) + f[v[k] as usize];
    }
}

/// `tinysdf::edt`: 2D Euclidean distance transform.
#[allow(clippy::too_many_arguments)]
fn edt(data: &mut [f64], width: u32, height: u32, f: &mut [f64], d: &mut [f64], v: &mut [i16], z: &mut [f64]) {
    for x in 0..width {
        for y in 0..height {
            f[y as usize] = data[(y * width + x) as usize];
        }
        edt1d(f, d, v, z, height);
        for y in 0..height {
            data[(y * width + x) as usize] = d[y as usize];
        }
    }
    for y in 0..height {
        for x in 0..width {
            f[x as usize] = data[(y * width + x) as usize];
        }
        edt1d(f, d, v, z, width);
        for x in 0..width {
            data[(y * width + x) as usize] = d[x as usize].sqrt();
        }
    }
}

/// `transformRasterToSDF`: an alpha raster to a signed distance field of
/// the same size.
#[must_use]
pub fn transform_raster_to_sdf(raster_input: &AlphaImage, radius: f64, cutoff: f64) -> AlphaImage {
    let size = raster_input.size.width * raster_input.size.height;
    let max_dimension = raster_input.size.width.max(raster_input.size.height);

    let mut sdf = AlphaImage::new(raster_input.size);

    // temporary arrays for the distance transform
    let mut grid_outer = vec![0f64; size as usize];
    let mut grid_inner = vec![0f64; size as usize];
    let mut f = vec![0f64; max_dimension as usize];
    let mut d = vec![0f64; max_dimension as usize];
    let mut z = vec![0f64; max_dimension as usize + 1];
    let mut v = vec![0i16; max_dimension as usize];

    for i in 0..size as usize {
        let a = f64::from(raster_input.data[i]) / 255.0; // alpha value
        grid_outer[i] = if a == 1.0 {
            0.0
        } else if a == 0.0 {
            INF
        } else {
            (0.0f64).max(0.5 - a).powf(2.0)
        };
        grid_inner[i] = if a == 1.0 {
            INF
        } else if a == 0.0 {
            0.0
        } else {
            (0.0f64).max(a - 0.5).powf(2.0)
        };
    }

    edt(&mut grid_outer, raster_input.size.width, raster_input.size.height, &mut f, &mut d, &mut v, &mut z);
    edt(&mut grid_inner, raster_input.size.width, raster_input.size.height, &mut f, &mut d, &mut v, &mut z);

    for i in 0..size as usize {
        let distance = grid_outer[i] - grid_inner[i];
        // std::max(0l, std::min(255l, ::lround(255.0 - 255.0 * (distance / radius + cutoff))))
        let r = lround(255.0 - 255.0 * (distance / radius + cutoff));
        sdf.data[i] = r.clamp(0, 255) as u8;
    }

    sdf
}

/// `::lround` (halfway cases away from zero; out-of-range values give the
/// x86-64 result, `LONG_MIN`).
fn lround(x: f64) -> i64 {
    let r = x.round();
    if (-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&r) { r as i64 } else { i64::MIN }
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
}
