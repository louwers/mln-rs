use super::*;

const MAPBOX: RasterEncoding = RasterEncoding::Mapbox;
const TERRARIUM: RasterEncoding = RasterEncoding::Terrarium;

#[test]
fn mapbox_decodes_its_extremes_exactly() {
    let unpack = DemUnpack::of(MAPBOX);
    assert_eq!(unpack.decode([0, 0, 0, 255]), -10_000.0);
    assert_eq!(unpack.decode([255, 255, 255, 255]), 1_667_721.5);
    // 65536 + 134 · 256 + 160 = 100000: sea level.
    assert_eq!(unpack.decode([1, 134, 160, 255]), 0.0);
    assert_eq!(unpack.decode([1, 134, 170, 0]), 1.0, "alpha is ignored");
}

#[test]
fn terrarium_decodes_its_extremes_exactly() {
    let unpack = DemUnpack::of(TERRARIUM);
    assert_eq!(unpack.decode([0, 0, 0, 255]), -32_768.0);
    assert_eq!(unpack.decode([255, 255, 255, 255]), 8_388_607.0 / 256.0);
    assert_eq!(unpack.decode([128, 0, 0, 255]), 0.0);
    assert_eq!(unpack.decode([128, 0, 128, 255]), 0.5);
    assert_eq!(unpack.decode([127, 255, 0, 255]), -1.0);
}

/// Every Terrarium texel decodes to its exact value: the sum is an integer
/// below 2^24 and the scale a power of two.
#[test]
fn terrarium_decodes_every_texel_exactly() {
    let unpack = DemUnpack::of(TERRARIUM);
    for r in 0..=255u8 {
        for g in (0..=255u8).step_by(15) {
            for b in [0, 1, 127, 128, 254, 255] {
                let exact = (f64::from(r) * 256.0 + f64::from(g) + f64::from(b) / 256.0) - 32_768.0;
                assert_eq!(f64::from(unpack.decode([r, g, b, 255])), exact, "{r} {g} {b}");
            }
        }
    }
}

/// A Mapbox height is the exact integer sum, scaled once.
#[test]
fn mapbox_rounds_once() {
    let unpack = DemUnpack::of(MAPBOX);
    for r in [0u8, 1, 100, 200, 255] {
        for g in [0u8, 1, 128, 255] {
            for b in [0u8, 1, 128, 255] {
                let sum = i32::from(r) * 65536 + i32::from(g) * 256 + i32::from(b) - 100_000;
                assert_eq!(unpack.decode([r, g, b, 255]), sum as f32 * 0.1, "{r} {g} {b}");
            }
        }
    }
}

#[test]
fn custom_decodes_with_its_factors() {
    let encoding =
        RasterEncoding::Custom { red_factor: 100.0, green_factor: 10.0, blue_factor: 0.5, base_shift: 1000.0 };
    assert_eq!(DemUnpack::of(encoding).decode([10, 20, 30, 255]), 215.0);
}

/// A DEM whose texels are a plane in pixel coordinates, `100·x + 10·y` at
/// pixel (x, y), so that bilinear interpolation reads the plane exactly.
fn plane(x: u32, y: u32) -> f32 {
    100.0 * (x as f32 - 1.0) + 10.0 * (y as f32 - 1.0)
}

/// GL JS's 'getElevation interpolates DEM cell centres': at uv 0 the
/// sample is half a pixel before the first pixel's centre, between it and
/// the border.
#[test]
fn samples_between_cell_centres() {
    let at = |uv| sample(2, DemCoords::OWN, uv, plane);
    assert_eq!(at([0.0, 0.0]), -55.0);
    assert_eq!(at([0.75, 0.25]), 100.0);
    // GL JS's 110 is this with an exaggeration of 2, which the caller
    // applies.
    assert_eq!(at([0.5, 0.5]), 55.0);
}

/// GL JS's 'getDEMElevation samples parent DEM cell centres': the centre of
/// tile 2/3/3 in its parent 1/1/1's DEM of 4 pixels is at pixel 2.5.
#[test]
fn samples_an_ancestors_sub_rect() {
    let coords = DemCoords { scale: 0.5, offset: [0.5, 0.5] };
    assert_eq!(sample(4, coords, [0.5, 0.5], plane), 275.0);
}

#[test]
fn samples_pixel_centres_exactly() {
    let dim = 4;
    let texel = |x: u32, y: u32| (x * 7 + y * 13) as f32;
    for i in 0..dim {
        for j in 0..dim {
            let uv = [(i as f32 + 0.5) / dim as f32, (j as f32 + 0.5) / dim as f32];
            assert_eq!(sample(dim, DemCoords::OWN, uv, texel), texel(i + 1, j + 1), "pixel {i} {j}");
        }
    }
}

/// At the tile's edges the sample is halfway to the border texel, which
/// the neighbour's edge fills.
#[test]
fn edges_read_the_border() {
    let dim = 4;
    // Every texel is 0 but the border's, which are 8.
    let texel = |x: u32, y: u32| if x == 0 || y == 0 || x == dim + 1 || y == dim + 1 { 8.0 } else { 0.0 };
    let at = |uv| sample(dim, DemCoords::OWN, uv, texel);
    assert_eq!(at([0.0, 0.5]), 4.0);
    assert_eq!(at([1.0, 0.5]), 4.0);
    assert_eq!(at([0.5, 0.0]), 4.0);
    assert_eq!(at([0.5, 1.0]), 4.0);
    // At a corner three of the four taps are border texels.
    assert_eq!(at([0.0, 0.0]), 6.0);
    assert_eq!(at([1.0, 1.0]), 6.0);
}

/// Taps past the bordered texels clamp to them.
#[test]
fn taps_clamp_to_the_border() {
    let seen = std::cell::RefCell::new(Vec::new());
    let texel = |x: u32, y: u32| {
        seen.borrow_mut().push((x, y));
        0.0
    };
    for uv in [[-1.0, -1.0], [2.0, 2.0], [1.0, 1.0], [0.0, 0.0]] {
        sample(4, DemCoords::OWN, uv, texel);
    }
    assert!(seen.borrow().iter().all(|&(x, y)| x <= 5 && y <= 5), "{:?}", seen.borrow());
}

#[test]
fn no_dem_is_flat() {
    assert_eq!(sample(0, DemCoords::OWN, [0.5, 0.5], |_, _| panic!("no texels")), 0.0);
}
