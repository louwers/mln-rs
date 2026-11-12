// Ports src/mln/geometry/dem_data.hpp and src/mln/geometry/dem_data.cpp
//
// C++ keeps the image behind a `shared_ptr` that textures share; here the
// image is owned and textures get a copy (`get_image`), because the DEM
// lives in a bucket shared between threads.

use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::tileset::RasterEncoding;
use serde::{Deserialize, Serialize};

/// https://www.mapbox.com/help/access-elevation-data/#mapbox-terrain-rgb
const UNPACK_MAPBOX: [f32; 4] = [6553.6, 25.6, 0.1, 10000.0];
/// https://aws.amazon.com/public-datasets/terrain/
const UNPACK_TERRARIUM: [f32; 4] = [256.0, 1.0, 1.0 / 256.0, 32768.0];

/// The DEM unpack vector for an encoding (`DEMData::getUnpackVector`): the
/// red, green and blue factors and the base shift.
pub fn get_unpack_vector(encoding: RasterEncoding) -> [f32; 4] {
    match encoding {
        RasterEncoding::Mapbox => UNPACK_MAPBOX,
        RasterEncoding::Terrarium => UNPACK_TERRARIUM,
        RasterEncoding::Custom { red_factor, green_factor, blue_factor, base_shift } => {
            [red_factor, green_factor, blue_factor, base_shift]
        }
    }
}

/// Port-only: serde reads back only what `new` can make (`from_parts`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "DEMDataParts")]
pub struct DEMData {
    pub dim: i32,
    pub stride: i32,
    pub encoding: RasterEncoding,
    image: PremultipliedImage,
}

/// What `DEMData` is read from.
#[derive(Deserialize)]
struct DEMDataParts {
    dim: i32,
    stride: i32,
    encoding: RasterEncoding,
    image: PremultipliedImage,
}

impl TryFrom<DEMDataParts> for DEMData {
    type Error = String;

    fn try_from(parts: DEMDataParts) -> Result<Self, String> {
        DEMData::from_parts(parts.dim, parts.stride, parts.encoding, parts.image)
    }
}

/// A 4-byte pixel at pixel index `i`.
fn pixel(data: &[u8], i: usize) -> [u8; 4] {
    [data[i * 4], data[i * 4 + 1], data[i * 4 + 2], data[i * 4 + 3]]
}

fn set_pixel(data: &mut [u8], i: usize, value: [u8; 4]) {
    data[i * 4..i * 4 + 4].copy_from_slice(&value);
}

impl DEMData {
    /// `DEMData(const PremultipliedImage&, Tileset::RasterEncoding)`. C++
    /// throws `std::runtime_error("raster-dem tiles must be square.")`.
    pub fn new(image: &PremultipliedImage, encoding: RasterEncoding) -> Result<Self, String> {
        let dim = image.size.height as i32;
        // extra two pixels per row for border backfilling on either edge
        let stride = dim + 2;
        let mut out = PremultipliedImage::new(Size::new(stride as u32, stride as u32));
        if image.size.height != image.size.width {
            return Err("raster-dem tiles must be square.".to_owned());
        }

        let row_bytes = dim as usize * 4;
        let stride_u = stride as usize;
        let dim_u = dim as usize;
        for y in 0..dim_u {
            let dest = ((y + 1) * stride_u + 1) * 4;
            let source = y * row_bytes;
            out.data[dest..dest + row_bytes].copy_from_slice(&image.data[source..source + row_bytes]);
        }

        // in order to avoid flashing seams between tiles, here we are
        // initially populating a 1px border of pixels around the image with
        // the data of the nearest pixel from the image. this data is
        // eventually replaced when the tile's neighboring tiles are loaded
        // and the accurate data can be backfilled using
        // DEMData#backfillBorder

        let data = &mut out.data;
        for x in 0..dim_u {
            let row_offset = stride_u * (x + 1);
            // left vertical border
            let left = pixel(data, row_offset + 1);
            set_pixel(data, row_offset, left);

            // right vertical border
            let right = pixel(data, row_offset + dim_u);
            set_pixel(data, row_offset + dim_u + 1, right);
        }

        // top horizontal border with corners
        data.copy_within(stride_u * 4..2 * stride_u * 4, 0);
        // bottom horizontal border with corners
        data.copy_within(dim_u * stride_u * 4..(dim_u + 1) * stride_u * 4, (dim_u + 1) * stride_u * 4);

        Ok(DEMData { dim, stride, encoding, image: out })
    }

    /// Port-only: a DEM as another `DEMData` was (its members), as serde
    /// reads it. Fails unless the image is `stride` by `stride` pixels with
    /// `stride == dim + 2` and `dim >= 0`, as `new` makes them. The sizes are
    /// checked without overflow: on wasm32 `Image::bytes` would wrap for a
    /// large stride.
    fn from_parts(dim: i32, stride: i32, encoding: RasterEncoding, image: PremultipliedImage) -> Result<Self, String> {
        let side = u32::try_from(stride).map_err(|_| format!("a DEM stride of {stride}"))?;
        let bytes = u64::from(side).checked_mul(u64::from(side)).and_then(|n| n.checked_mul(4));
        if dim < 0
            || dim.checked_add(2) != Some(stride)
            || image.size != Size::new(side, side)
            || bytes != Some(image.data.len() as u64)
        {
            return Err(format!(
                "a {}x{} DEM image of {} bytes for dim {dim}, stride {stride}",
                image.size.width,
                image.size.height,
                image.data.len()
            ));
        }
        Ok(DEMData { dim, stride, encoding, image })
    }

    /// `idx(x, y)`.
    fn idx(&self, x: i32, y: i32) -> usize {
        debug_assert!(x >= -1);
        debug_assert!(x < self.dim + 1);
        debug_assert!(y >= -1);
        debug_assert!(y < self.dim + 1);
        ((y + 1) * self.stride + (x + 1)) as usize
    }

    /// This function takes the DEMData from a neighboring tile and
    /// backfills the edge/corner data in order to create a one pixel
    /// "buffer" of image data around the tile. This is necessary because
    /// the hillshade formula calculates the dx/dz, dy/dz derivatives at each
    /// pixel of the tile by querying the 8 surrounding pixels, and if we
    /// don't have the pixel buffer we get seams at tile boundaries.
    pub fn backfill_border(&mut self, border_tile_data: &DEMData, dx: i8, dy: i8) {
        let o = border_tile_data;

        // Tiles from the same source should always be of the same dimensions.
        // C++ asserts it and reads out of bounds when they aren't (tiles of
        // different sizes, or an empty one from a corrupt image); the port
        // leaves the border as it is.
        if self.dim != o.dim || self.dim == 0 {
            return;
        }

        // We determine the pixel range to backfill based which corner/edge
        // `borderTileData` represents. For example, dx = -1, dy = -1
        // represents the upper left corner of the base tile, so we only
        // need to backfill one pixel at coordinates (-1, -1) of the tile
        // image.
        let (dx, dy) = (i32::from(dx), i32::from(dy));
        let dim = self.dim;
        let mut x_min = dx * dim;
        let mut x_max = dx * dim + dim;
        let mut y_min = dy * dim;
        let mut y_max = dy * dim + dim;

        if dx == -1 {
            x_min = x_max - 1;
        } else if dx == 1 {
            x_max = x_min + 1;
        }

        if dy == -1 {
            y_min = y_max - 1;
        } else if dy == 1 {
            y_max = y_min + 1;
        }

        let ox = -dx * dim;
        let oy = -dy * dim;

        for y in y_min..y_max {
            for x in x_min..x_max {
                let value = pixel(&o.image.data, o.idx(x + ox, y + oy));
                let i = self.idx(x, y);
                set_pixel(&mut self.image.data, i, value);
            }
        }
    }

    /// `get(x, y)`.
    pub fn get(&self, x: i32, y: i32) -> i32 {
        let unpack = self.get_unpack_vector();
        let value = pixel(&self.image.data, self.idx(x, y));
        (f32::from(value[0]) * unpack[0] + f32::from(value[1]) * unpack[1] + f32::from(value[2]) * unpack[2]
            - unpack[3]) as i32
    }

    /// `getUnpackVector()`.
    pub fn get_unpack_vector(&self) -> [f32; 4] {
        get_unpack_vector(self.encoding)
    }

    /// `getImage()`.
    pub fn get_image(&self) -> &PremultipliedImage {
        &self.image
    }
}

#[cfg(test)]
mod tests {
    // Ports test/geometry/dem_data.test.cpp.
    use super::*;

    /// `fakeImage(Size)`: alpha 1, other channels pseudo-random (C++
    /// `std::rand() % 255`; any values do).
    fn fake_image(size: Size) -> PremultipliedImage {
        let mut img = PremultipliedImage::new(size);
        let mut state: u32 = 12345;
        for i in 0..img.bytes() {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            img.data[i] = if (i + 1) % 4 == 0 { 1 } else { ((state >> 16) % 255) as u8 };
        }
        img
    }

    // gtest: DEMData.ConstructorMapbox
    #[test]
    fn constructor_mapbox() {
        let image = fake_image(Size::new(16, 16));
        let demdata = DEMData::new(&image, RasterEncoding::Mapbox).unwrap();

        assert_eq!(demdata.dim, 16);
        assert_eq!(demdata.stride, 18);
        assert_eq!(demdata.get_image().bytes(), 18 * 18 * 4);
    }

    // gtest: DEMData.ConstructorTerrarium
    #[test]
    fn constructor_terrarium() {
        let image = fake_image(Size::new(16, 16));
        let demdata = DEMData::new(&image, RasterEncoding::Terrarium).unwrap();

        assert_eq!(demdata.dim, 16);
        assert_eq!(demdata.stride, 18);
        assert_eq!(demdata.get_image().bytes(), 18 * 18 * 4);
    }

    #[test]
    fn custom_encoding_decodes_with_its_factors() {
        let mut image = PremultipliedImage::new(Size::new(1, 1));
        image.data.copy_from_slice(&[10, 20, 30, 255]);
        let encoding =
            RasterEncoding::Custom { red_factor: 100.0, green_factor: 10.0, blue_factor: 0.5, base_shift: 1000.0 };
        let demdata = DEMData::new(&image, encoding).unwrap();

        // 10 * 100 + 20 * 10 + 30 * 0.5 - 1000
        assert_eq!(demdata.get(0, 0), 215);
        assert_eq!(demdata.get_unpack_vector(), [100.0, 10.0, 0.5, 1000.0]);
    }

    #[test]
    fn not_square() {
        let image = fake_image(Size::new(16, 8));
        assert_eq!(DEMData::new(&image, RasterEncoding::Mapbox).unwrap_err(), "raster-dem tiles must be square.");
    }

    // gtest: DEMData.InitialBackfill
    #[test]
    fn initial_backfill() {
        let image1 = fake_image(Size::new(4, 4));
        let dem1 = DEMData::new(&image1, RasterEncoding::Mapbox).unwrap();

        // checking that a 1 px border around the fake image has been
        // populated with a non-empty pixel value
        let mut nonempty = true;
        for x in -1..5 {
            for y in -1..5 {
                if dem1.get(x, y) == -65536 {
                    nonempty = false;
                }
            }
        }
        assert!(nonempty);

        // vertical border of DEM data is initially equal to next column of
        // data
        for x in [-1, 4] {
            for y in 0..4 {
                assert_eq!(dem1.get(x, y), dem1.get(if x < 0 { x + 1 } else { x - 1 }, y));
            }
        }

        // horizontal border of DEM data is initially equal to next row of
        // data
        for y in [-1, 4] {
            for x in 0..4 {
                assert_eq!(dem1.get(x, y), dem1.get(x, if y < 0 { y + 1 } else { y - 1 }));
            }
        }

        // corners initially equal to closest corner data
        assert_eq!(dem1.get(-1, 4), dem1.get(0, 3));
        assert_eq!(dem1.get(4, 4), dem1.get(3, 3));
        assert_eq!(dem1.get(-1, -1), dem1.get(0, 0));
        assert_eq!(dem1.get(4, -1), dem1.get(3, 0));
    }

    // gtest: DEMData.BackfillNeighbor
    #[test]
    fn backfill_neighbor() {
        let image1 = fake_image(Size::new(4, 4));
        let mut dem0 = DEMData::new(&image1, RasterEncoding::Mapbox).unwrap();
        let image2 = fake_image(Size::new(4, 4));
        let dem1 = DEMData::new(&image2, RasterEncoding::Mapbox).unwrap();

        dem0.backfill_border(&dem1, -1, 0);
        for y in 0..4 {
            // dx = -1, dy = 0, so the left edge of dem1 should equal the
            // right edge of dem0 backfills Left neighbor
            assert_eq!(dem0.get(-1, y), dem1.get(3, y));
        }

        dem0.backfill_border(&dem1, 0, -1);
        // backfills TopCenter neighbor
        for x in 0..4 {
            assert_eq!(dem0.get(x, -1), dem1.get(x, 3));
        }

        dem0.backfill_border(&dem1, 1, 0);
        // backfills Right neighbor
        for y in 0..4 {
            assert_eq!(dem0.get(4, y), dem1.get(0, y));
        }

        dem0.backfill_border(&dem1, 0, 1);
        // backfills BottomCenter neighbor
        for x in 0..4 {
            assert_eq!(dem0.get(x, 4), dem1.get(x, 0));
        }

        dem0.backfill_border(&dem1, -1, 1);
        assert_eq!(dem0.get(-1, 4), dem1.get(3, 0));

        dem0.backfill_border(&dem1, 1, 1);
        assert_eq!(dem0.get(4, 4), dem1.get(0, 0));

        dem0.backfill_border(&dem1, -1, -1);
        assert_eq!(dem0.get(-1, -1), dem1.get(3, 3));

        dem0.backfill_border(&dem1, 1, -1);
        assert_eq!(dem0.get(4, -1), dem1.get(0, 3));
    }

    // Port-only: C++ reads out of bounds for neighbours of another size.
    #[test]
    fn backfill_border_of_another_size() {
        let mut dem = DEMData::new(&fake_image(Size::new(4, 4)), RasterEncoding::Mapbox).unwrap();
        let border = dem.get(-1, -1);
        let larger = DEMData::new(&fake_image(Size::new(8, 8)), RasterEncoding::Mapbox).unwrap();
        let empty = DEMData::new(&PremultipliedImage::default(), RasterEncoding::Mapbox).unwrap();
        for (dx, dy) in [(-1, -1), (0, 1), (1, 0)] {
            dem.backfill_border(&larger, dx, dy);
            dem.backfill_border(&empty, dx, dy);
        }
        assert_eq!(dem.get(-1, -1), border);
        let mut empty = empty;
        empty.backfill_border(&empty.clone(), -1, -1);
    }
}
