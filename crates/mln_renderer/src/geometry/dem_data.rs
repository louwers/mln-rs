// The DEM shares its image, which textures copy (`image`): the DEM crosses
// from the workers, so it is Send, and textures live on the render thread. A
// backfill copies the image first if anything else holds it.

use std::sync::{Arc, OnceLock};

use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;
use mln_style::tileset::RasterEncoding;
use mln_util::dem::DemUnpack;

/// A raster-dem tile's elevations, a pixel of border on each side. It reads
/// back only as `new` can make it (`from_parts`).
#[derive(Clone, Debug)]
pub(crate) struct DEMData {
    pub(crate) dim: i32,
    pub(crate) stride: i32,
    pub(crate) encoding: RasterEncoding,
    /// [`DEMData::range`], once read: only the terrain reads it, so a map
    /// without terrain never decodes its DEM tiles' heights.
    range: OnceLock<(f32, f32)>,
    image: Arc<PremultipliedImage>,
}

impl mln_wire::Wire for DEMData {
    fn write(&self, w: &mut mln_wire::Writer) {
        self.dim.write(w);
        self.stride.write(w);
        self.encoding.write(w);
        self.image.write(w);
    }

    fn read(r: &mut mln_wire::Reader<'_>) -> Option<Self> {
        let dim = i32::read(r)?;
        let stride = i32::read(r)?;
        let encoding = RasterEncoding::read(r)?;
        let image = PremultipliedImage::read(r)?;
        DEMData::from_parts(dim, stride, encoding, image).ok().or_else(|| r.fail("DEM data that `new` can't make"))
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
    /// Decodes a DEM image with `encoding`; `Err` if the image isn't square.
    pub(crate) fn new(image: &PremultipliedImage, encoding: RasterEncoding) -> Result<Self, String> {
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
        // `backfill_border`.

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

        Ok(DEMData { dim, stride, encoding, range: OnceLock::new(), image: Arc::new(out) })
    }

    /// A DEM as another `DEMData` was (its members), as it reads back.
    /// Fails unless the image is `stride` by `stride` pixels with
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
        Ok(DEMData { dim, stride, encoding, range: OnceLock::new(), image: Arc::new(image) })
    }

    /// The lowest and highest elevation of the tile's own pixels, without
    /// the border, in meters; 0 for a DEM of no pixels. Decoded on the first
    /// call.
    pub(crate) fn range(&self) -> (f32, f32) {
        *self.range.get_or_init(|| {
            let unpack = DemUnpack::of(self.encoding);
            let (dim, stride) = (self.dim as usize, self.stride as usize);
            (0..dim)
                .flat_map(|y| {
                    let row = ((y + 1) * stride + 1) * 4;
                    self.image.data[row..row + dim * 4].as_chunks::<4>().0
                })
                .map(|&texel| unpack.decode(texel))
                .fold(None, |range: Option<(f32, f32)>, height| match range {
                    Some((min, max)) => Some((min.min(height), max.max(height))),
                    None => Some((height, height)),
                })
                .unwrap_or((0.0, 0.0))
        })
    }

    /// `idx(x, y)`.
    fn idx(&self, x: i32, y: i32) -> usize {
        debug_assert!(x >= -1);
        debug_assert!(x < self.dim + 1);
        debug_assert!(y >= -1);
        debug_assert!(y < self.dim + 1);
        ((y + 1) * self.stride + (x + 1)) as usize
    }

    /// This function takes the `DEMData` from a neighboring tile and
    /// backfills the edge/corner data in order to create a one pixel
    /// "buffer" of image data around the tile. This is necessary because
    /// the hillshade formula calculates the dx/dz, dy/dz derivatives at each
    /// pixel of the tile by querying the 8 surrounding pixels, and if we
    /// don't have the pixel buffer we get seams at tile boundaries.
    pub(crate) fn backfill_border(&mut self, border_tile_data: &DEMData, dx: i8, dy: i8) {
        let o = border_tile_data;

        // Tiles from the same source should always be of the same
        // dimensions. Where they aren't (tiles of different sizes, or an
        // empty one from a corrupt image), the border stays as it is.
        if self.dim != o.dim || self.dim == 0 {
            return;
        }

        // We determine the pixel range to backfill based which corner/edge
        // `border_tile_data` represents. For example, dx = -1, dy = -1
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

        debug_assert!(self.idx(x_min, y_min) <= self.idx(x_max - 1, y_max - 1));
        let (stride, image) = (self.stride, Arc::make_mut(&mut self.image));
        for y in y_min..y_max {
            for x in x_min..x_max {
                let value = pixel(&o.image.data, o.idx(x + ox, y + oy));
                set_pixel(&mut image.data, ((y + 1) * stride + (x + 1)) as usize, value);
            }
        }
    }

    /// The height in meters of the pixel at `x`, `y`, each from -1 to
    /// `dim`, the border's included.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn get(&self, x: i32, y: i32) -> f32 {
        DemUnpack::of(self.encoding).decode(pixel(&self.image.data, self.idx(x, y)))
    }

    #[must_use]
    pub(crate) fn image(&self) -> &PremultipliedImage {
        &self.image
    }

    /// The bordered image, to share: a backfill after copies it first.
    #[must_use]
    pub(crate) fn shared_image(&self) -> &Arc<PremultipliedImage> {
        &self.image
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image with alpha 1 and pseudo-random other channels (any values
    /// do).
    fn fake_image(size: Size) -> PremultipliedImage {
        let mut img = PremultipliedImage::new(size);
        let mut state: u32 = 12345;
        for i in 0..img.bytes() {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
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
        assert_eq!(demdata.image().bytes(), 18 * 18 * 4);
    }

    // gtest: DEMData.ConstructorTerrarium
    #[test]
    fn constructor_terrarium() {
        let image = fake_image(Size::new(16, 16));
        let demdata = DEMData::new(&image, RasterEncoding::Terrarium).unwrap();

        assert_eq!(demdata.dim, 16);
        assert_eq!(demdata.stride, 18);
        assert_eq!(demdata.image().bytes(), 18 * 18 * 4);
    }

    #[test]
    fn custom_encoding_decodes_with_its_factors() {
        let mut image = PremultipliedImage::new(Size::new(1, 1));
        image.data.copy_from_slice(&[10, 20, 30, 255]);
        let encoding =
            RasterEncoding::Custom { red_factor: 100.0, green_factor: 10.0, blue_factor: 0.5, base_shift: 1000.0 };
        let demdata = DEMData::new(&image, encoding).unwrap();

        // 10 * 100 + 20 * 10 + 30 * 0.5 - 1000
        assert_eq!(demdata.get(0, 0), 215.0);
    }

    /// The range is of the tile's own pixels, not its border, which a
    /// neighbour's lower or higher edge fills.
    #[test]
    fn range_of_the_interior() {
        // Terrarium heights 0, 1, 2, 3 m (r 128, g = height).
        let mut image = PremultipliedImage::new(Size::new(2, 2));
        for (i, texel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *texel = [128, i as u8, 0, 255];
        }
        let mut dem = DEMData::new(&image, RasterEncoding::Terrarium).unwrap();
        assert_eq!(dem.range.get(), None, "decoded only when read");
        assert_eq!(dem.range(), (0.0, 3.0));

        let mut deep = PremultipliedImage::new(Size::new(2, 2));
        deep.data.copy_from_slice(&[[0, 0, 0, 255]; 4].concat());
        let deep = DEMData::new(&deep, RasterEncoding::Terrarium).unwrap();
        let shared = dem.clone();
        dem.backfill_border(&deep, -1, 0);
        assert_eq!(dem.range(), (0.0, 3.0));
        let mut fresh = DEMData::new(&image, RasterEncoding::Terrarium).unwrap();
        fresh.backfill_border(&deep, -1, 0);
        assert_eq!(fresh.range(), (0.0, 3.0), "a range read after a backfill");
        assert_eq!(dem.get(-1, 0), -32768.0);
        assert_eq!(shared.get(-1, 0), 0.0, "a backfill copies a shared image");

        let empty = DEMData::new(&PremultipliedImage::default(), RasterEncoding::Mapbox).unwrap();
        assert_eq!(empty.range(), (0.0, 0.0));
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
                if dem1.get(x, y) == -65536.0 {
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

    // A neighbour of another size leaves the border as it is.
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
