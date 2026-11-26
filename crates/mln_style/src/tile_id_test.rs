use super::*;

fn c(z: u8, x: u32, y: u32) -> CanonicalTileID {
    CanonicalTileID::new(z, x, y)
}

// gtest: TileID.Canonical
#[test]
fn canonical() {
    let id_0_0_0 = c(0, 0, 0);
    assert_eq!(0, id_0_0_0.z);
    assert_eq!(0, id_0_0_0.x);
    assert_eq!(0, id_0_0_0.y);

    assert_eq!(c(4, 2, 3), c(4, 2, 3));
    assert_ne!(c(4, 2, 3), c(5, 2, 3));
    assert_ne!(c(4, 2, 3), c(4, 3, 3));
    assert_ne!(c(4, 2, 3), c(4, 2, 4));
    assert_ne!(c(5, 2, 3), c(4, 2, 3));
    assert_ne!(c(4, 3, 3), c(4, 2, 3));
    assert_ne!(c(4, 2, 4), c(4, 2, 3));

    assert!(c(0, 0, 0) < c(1, 0, 0));
    assert!(c(1, 0, 0) >= c(0, 0, 0));
    assert!(c(4, 2, 3) < c(4, 3, 2));
    assert!(c(4, 3, 2) >= c(4, 2, 3));
    assert!(c(5, 3, 6) >= c(5, 3, 6));

    let child = c(4, 2, 3);
    assert!(child.is_child_of(&c(3, 1, 1)));
    assert!(child.is_child_of(&c(2, 0, 0)));
    assert!(child.is_child_of(&c(1, 0, 0)));
    assert!(child.is_child_of(&c(0, 0, 0)));
    assert!(!child.is_child_of(&c(5, 4, 6)));
    assert!(!child.is_child_of(&c(4, 2, 4)));
    assert!(!child.is_child_of(&c(3, 2, 2)));
    assert!(!child.is_child_of(&c(2, 1, 0)));
    assert!(!child.is_child_of(&c(1, 0, 1)));

    assert_eq!(0, 4_294_967_295_u64 >> 32);

    let max_child = c(32, 4_294_967_295, 4_294_967_295);
    assert!(!max_child.is_child_of(&c(32, 4_294_967_295, 4_294_967_295)));
    let mut parent_xy: u32 = 2_147_483_647;
    for z in (1..=31u8).rev() {
        assert!(max_child.is_child_of(&c(z, parent_xy, parent_xy)), "z = {z}");
        parent_xy >>= 1;
    }
    assert!(max_child.is_child_of(&c(0, 0, 0)));

    let scale_base = c(4, 5, 13);
    assert_eq!(c(0, 0, 0), scale_base.scaled_to(0));
    assert_eq!(c(1, 0, 1), scale_base.scaled_to(1));
    assert_eq!(c(2, 1, 3), scale_base.scaled_to(2));
    assert_eq!(c(3, 2, 6), scale_base.scaled_to(3));
    assert_eq!(c(4, 5, 13), scale_base.scaled_to(4));
    assert_eq!(c(5, 10, 26), scale_base.scaled_to(5));
    assert_eq!(c(6, 20, 52), scale_base.scaled_to(6));
    assert_eq!(c(7, 40, 104), scale_base.scaled_to(7));
    assert_eq!(c(8, 80, 208), scale_base.scaled_to(8));
    assert_eq!(c(9, 160, 416), scale_base.scaled_to(9));
    assert_eq!(c(10, 320, 832), scale_base.scaled_to(10));
    assert_eq!(c(11, 640, 1664), scale_base.scaled_to(11));
    assert_eq!(c(12, 1280, 3328), scale_base.scaled_to(12));
    assert_eq!(c(13, 2560, 6656), scale_base.scaled_to(13));
    assert_eq!(c(14, 5120, 13312), scale_base.scaled_to(14));
    assert_eq!(c(15, 10240, 26624), scale_base.scaled_to(15));
    assert_eq!(c(16, 20480, 53248), scale_base.scaled_to(16));
    assert_eq!(c(17, 40960, 106_496), scale_base.scaled_to(17));
    assert_eq!(c(18, 81920, 212_992), scale_base.scaled_to(18));
    assert_eq!(c(19, 163_840, 425_984), scale_base.scaled_to(19));
    assert_eq!(c(20, 327_680, 851_968), scale_base.scaled_to(20));
    assert_eq!(c(21, 655_360, 1_703_936), scale_base.scaled_to(21));
    assert_eq!(c(22, 1_310_720, 3_407_872), scale_base.scaled_to(22));
    assert_eq!(c(23, 2_621_440, 6_815_744), scale_base.scaled_to(23));
    assert_eq!(c(24, 5_242_880, 13_631_488), scale_base.scaled_to(24));
    assert_eq!(c(25, 10_485_760, 27_262_976), scale_base.scaled_to(25));
    assert_eq!(c(26, 20_971_520, 54_525_952), scale_base.scaled_to(26));
    assert_eq!(c(27, 41_943_040, 109_051_904), scale_base.scaled_to(27));
    assert_eq!(c(28, 83_886_080, 218_103_808), scale_base.scaled_to(28));
    assert_eq!(c(29, 167_772_160, 436_207_616), scale_base.scaled_to(29));
    assert_eq!(c(30, 335_544_320, 872_415_232), scale_base.scaled_to(30));
    assert_eq!(c(31, 671_088_640, 1_744_830_464), scale_base.scaled_to(31));
    assert_eq!(c(32, 1_342_177_280, 3_489_660_928), scale_base.scaled_to(32));

    assert_eq!([c(5, 4, 6), c(5, 4, 7), c(5, 5, 6), c(5, 5, 7)], c(4, 2, 3).children());
    assert_eq!([c(1, 0, 0), c(1, 0, 1), c(1, 1, 0), c(1, 1, 1)], c(0, 0, 0).children());
}

// Not in the C++ test file: the `util::toString` / `operator<<` format from
// src/mln/tile/tile_id_io.cpp.
#[test]
fn to_string() {
    assert_eq!("4/2/3", c(4, 2, 3).to_string());
}
