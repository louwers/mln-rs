//! The height rule of terrain: how a DEM texel decodes to meters, and how a
//! height is read between texels. The CPU (camera, culling ranges) and every
//! shader language follow this rule, so the camera stands on the surface
//! that draws.
//!
//! A DEM tile of `dim` pixels a side is kept behind a one-texel border, so
//! its bordered texels run from 0 to `dim + 1` and pixel `i` is texel
//! `i + 1`. Pixel `i` describes the cell centred at `(i + 0.5) / dim` of the
//! tile, as hillshade and color-relief read it.

use crate::tileset::RasterEncoding;

/// How a DEM's texels decode: `(r·w0 + g·w1 + b·w2 + base) · scale`, over
/// channels from 0 to 255.
///
/// For Mapbox and Terrarium the weights and base are integers and the sum is
/// below 2^24, so the sum is exact in `f32` and the decode rounds once, the
/// same on the CPU and the GPU.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemUnpack {
    pub weights: [f32; 3],
    pub base: f32,
    pub scale: f32,
}

impl DemUnpack {
    /// The decode of `encoding`.
    #[must_use]
    pub fn of(encoding: RasterEncoding) -> DemUnpack {
        const WEIGHTS: [f32; 3] = [65536.0, 256.0, 1.0];
        match encoding {
            RasterEncoding::Mapbox => DemUnpack { weights: WEIGHTS, base: -100_000.0, scale: 0.1 },
            RasterEncoding::Terrarium => DemUnpack { weights: WEIGHTS, base: -8_388_608.0, scale: 1.0 / 256.0 },
            RasterEncoding::Custom { red_factor, green_factor, blue_factor, base_shift } => {
                DemUnpack { weights: [red_factor, green_factor, blue_factor], base: -base_shift, scale: 1.0 }
            }
        }
    }

    /// The height in meters of an RGBA `texel`; alpha is ignored.
    #[must_use]
    pub fn decode(&self, texel: [u8; 4]) -> f32 {
        let [r, g, b, _] = texel.map(f32::from);
        let [w0, w1, w2] = self.weights;
        (r * w0 + g * w1 + b * w2 + self.base) * self.scale
    }
}

/// Where a terrain tile reads its DEM: DEM uv = tile uv · `scale` +
/// `offset`. A tile reading its own DEM tile has scale 1 and offset 0; one
/// reading an ancestor's reads the sub-rect it covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemCoords {
    pub scale: f32,
    pub offset: [f32; 2],
}

impl DemCoords {
    /// The whole DEM tile.
    pub const OWN: DemCoords = DemCoords { scale: 1.0, offset: [0.0, 0.0] };
}

/// The unexaggerated height at `uv` (0 to 1 across the tile, y down) of a
/// DEM with `dim` pixels a side, read through `coords`. `texel(x, y)`
/// decodes the bordered texel at `x`, `y`, each in `0..=dim + 1`. A `dim` of
/// 0 is no DEM: the height is 0.
///
/// The height interpolates bilinearly, in `f32`, between the four texels
/// around the texel coordinate `(uv · scale + offset) · dim + 0.5`, each
/// clamped to the bordered texels.
pub fn sample(dim: u32, coords: DemCoords, uv: [f32; 2], texel: impl Fn(u32, u32) -> f32) -> f32 {
    if dim == 0 {
        return 0.0;
    }
    let last = i64::from(dim) + 1;
    let tap = |at: f32| {
        let floor = at.floor();
        let first = (floor as i64).clamp(0, last) as u32;
        let second = (floor as i64 + 1).clamp(0, last) as u32;
        (first, second, at - floor)
    };
    let dim = dim as f32;
    let (x0, x1, fx) = tap((uv[0] * coords.scale + coords.offset[0]) * dim + 0.5);
    let (y0, y1, fy) = tap((uv[1] * coords.scale + coords.offset[1]) * dim + 0.5);
    let mix = |a: f32, b: f32, t: f32| a * (1.0 - t) + b * t;
    let top = mix(texel(x0, y0), texel(x1, y0), fx);
    let bottom = mix(texel(x0, y1), texel(x1, y1), fx);
    mix(top, bottom, fy)
}

#[cfg(test)]
#[path = "dem_test.rs"]
mod dem_test;
