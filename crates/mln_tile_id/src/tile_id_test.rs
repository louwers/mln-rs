use super::*;

fn c(z: u8, x: u32, y: u32) -> CanonicalTileID {
    CanonicalTileID::new(z, x, y)
}

fn o(z: u8, x: u32, y: u32) -> OverscaledTileID {
    OverscaledTileID::from_zxy(z, x, y)
}

fn ow(overscaled_z: u8, wrap: i16, canonical: CanonicalTileID) -> OverscaledTileID {
    OverscaledTileID::new(overscaled_z, wrap, canonical)
}

fn u(z: u8, x: i64, y: i64) -> UnwrappedTileID {
    UnwrappedTileID::new(z, x, y)
}

fn uw(wrap: i16, canonical: CanonicalTileID) -> UnwrappedTileID {
    UnwrappedTileID::with_wrap(wrap, canonical)
}

// gtest: TileID.Overscaled
#[test]
fn overscaled() {
    assert_eq!(o(4, 2, 3), o(4, 2, 3));
    assert_eq!(ow(4, 0, c(4, 2, 3)), o(4, 2, 3));
    assert_eq!(o(4, 2, 3), ow(4, 0, c(4, 2, 3)));

    assert_ne!(o(4, 2, 3), ow(5, 0, c(4, 2, 3)));
    assert_ne!(o(4, 2, 3), ow(6, 0, c(4, 2, 3)));
    assert_ne!(o(4, 2, 3), ow(7, 0, c(4, 2, 3)));

    assert_ne!(o(4, 2, 3), o(4, 2, 4));
    assert_ne!(o(4, 2, 3), o(4, 3, 3));
    assert_ne!(o(4, 2, 3), o(5, 2, 3));

    assert_eq!(ow(7, 0, c(4, 2, 3)), ow(7, 0, c(4, 2, 3)));

    assert!(o(4, 2, 3) >= o(4, 2, 3));
    assert!(o(4, 2, 3) < ow(5, 0, c(4, 2, 3)));
    assert!(ow(5, 0, c(4, 2, 3)) >= o(4, 2, 3));
    assert!(o(4, 2, 3) < ow(6, 0, c(4, 2, 3)));
    assert!(ow(6, 0, c(4, 2, 3)) >= o(4, 2, 3));
    assert!(o(4, 2, 3) < ow(7, 0, c(4, 2, 3)));
    assert!(ow(7, 0, c(4, 2, 3)) >= o(4, 2, 3));

    assert_eq!(8, ow(7, 0, c(4, 2, 3)).overscale_factor());
    assert_eq!(4, ow(6, 0, c(4, 2, 3)).overscale_factor());
    assert_eq!(2, ow(5, 0, c(4, 2, 3)).overscale_factor());
    assert_eq!(1, ow(4, 0, c(4, 2, 3)).overscale_factor());
    assert_eq!(2_147_483_648, ow(31, 0, c(0, 0, 0)).overscale_factor());

    assert_eq!(ow(0, 0, c(0, 0, 0)), o(4, 2, 3).scaled_to(0));
    assert_eq!(ow(1, 0, c(1, 0, 0)), o(4, 2, 3).scaled_to(1));
    assert_eq!(ow(2, 0, c(2, 0, 0)), o(4, 2, 3).scaled_to(2));
    assert_eq!(ow(3, 0, c(3, 1, 1)), o(4, 2, 3).scaled_to(3));
    assert_eq!(ow(4, 0, c(4, 2, 3)), o(4, 2, 3).scaled_to(4));
    assert_eq!(ow(5, 0, c(4, 2, 3)), o(4, 2, 3).scaled_to(5));
    assert_eq!(ow(6, 0, c(4, 2, 3)), o(4, 2, 3).scaled_to(6));
    assert_eq!(ow(7, 0, c(4, 2, 3)), o(4, 2, 3).scaled_to(7));
    assert_eq!(ow(8, 0, c(4, 2, 3)), o(4, 2, 3).scaled_to(8));
    assert_eq!(ow(32, 0, c(4, 2, 3)), o(4, 2, 3).scaled_to(32));

    assert_eq!(uw(0, c(4, 2, 3)), o(4, 2, 3).to_unwrapped());
    assert_eq!(uw(-1, c(4, 2, 3)), ow(4, -1, c(4, 2, 3)).to_unwrapped());
    assert_eq!(uw(1, c(4, 2, 3)), ow(4, 1, c(4, 2, 3)).to_unwrapped());
    assert_eq!(uw(0, c(4, 2, 3)), ow(5, 0, c(4, 2, 3)).to_unwrapped());
    assert_eq!(uw(-1, c(4, 2, 3)), ow(5, -1, c(4, 2, 3)).to_unwrapped());
    assert_eq!(uw(1, c(4, 2, 3)), ow(5, 1, c(4, 2, 3)).to_unwrapped());

    assert!(!o(2, 0, 0).is_child_of(&o(3, 1, 1)));
    assert!(!o(3, 1, 1).is_child_of(&o(3, 1, 1)));
    assert!(o(4, 2, 3).is_child_of(&o(3, 1, 1)));
    assert!(ow(5, 0, c(4, 2, 3)).is_child_of(&o(3, 1, 1)));
    assert!(ow(6, 0, c(4, 2, 3)).is_child_of(&o(3, 1, 1)));
    assert!(!ow(6, -1, c(4, 2, 3)).is_child_of(&o(3, 1, 1)));
    assert!(ow(7, 0, c(4, 2, 3)).is_child_of(&o(3, 1, 1)));

    assert!(!o(2, 0, 0).is_child_of(&ow(5, 0, c(4, 2, 3))));
    assert!(!o(3, 1, 1).is_child_of(&ow(5, 0, c(4, 2, 3))));
    assert!(!o(4, 2, 3).is_child_of(&ow(5, 0, c(4, 2, 3))));
    assert!(!ow(5, 0, c(4, 2, 3)).is_child_of(&ow(5, 0, c(4, 2, 3))));
    assert!(ow(6, 0, c(4, 2, 3)).is_child_of(&ow(5, 0, c(4, 2, 3))));
    assert!(ow(7, 0, c(4, 2, 3)).is_child_of(&ow(5, 0, c(4, 2, 3))));
    assert!(!ow(7, 1, c(4, 2, 3)).is_child_of(&ow(5, 0, c(4, 2, 3))));

    assert!(!o(2, 0, 0).is_child_of(&ow(6, 0, c(4, 2, 3))));
    assert!(!o(3, 1, 1).is_child_of(&ow(6, 0, c(4, 2, 3))));
    assert!(!o(4, 2, 3).is_child_of(&ow(6, 0, c(4, 2, 3))));
    assert!(!ow(5, 0, c(4, 2, 3)).is_child_of(&ow(6, 0, c(4, 2, 3))));
    assert!(!ow(6, 0, c(4, 2, 3)).is_child_of(&ow(6, 0, c(4, 2, 3))));
    assert!(ow(7, 0, c(4, 2, 3)).is_child_of(&ow(6, 0, c(4, 2, 3))));
    assert!(!ow(7, 0, c(4, 2, 3)).is_child_of(&ow(6, -1, c(4, 2, 3))));

    assert!(!o(2, 0, 0).is_child_of(&ow(5, 0, c(4, 0, 0))));
    assert!(!o(3, 1, 1).is_child_of(&ow(5, 0, c(4, 0, 0))));
    assert!(!o(4, 2, 3).is_child_of(&ow(5, 0, c(4, 0, 0))));
    assert!(!ow(5, 0, c(4, 2, 3)).is_child_of(&ow(5, 0, c(4, 0, 0))));
    assert!(!ow(6, 0, c(4, 2, 3)).is_child_of(&ow(5, 0, c(4, 0, 0))));
    assert!(!ow(7, 0, c(4, 2, 3)).is_child_of(&ow(5, 0, c(4, 0, 0))));

    assert!(!ow(4, 0, c(4, 2, 3)).is_child_of(&ow(5, 0, c(3, 1, 1))));
}

// gtest: TileID.Unwrapped
#[test]
fn unwrapped() {
    let id_0_0_0 = u(0, 0, 0);
    assert_eq!(0, id_0_0_0.canonical.z);
    assert_eq!(0, id_0_0_0.canonical.x);
    assert_eq!(0, id_0_0_0.canonical.y);
    assert_eq!(0, id_0_0_0.wrap);

    let id_0_1_0 = u(0, 1, 0);
    assert_eq!(0, id_0_1_0.canonical.z);
    assert_eq!(0, id_0_1_0.canonical.x);
    assert_eq!(0, id_0_1_0.canonical.y);
    assert_eq!(1, id_0_1_0.wrap);

    let id_0_4_0 = u(0, 4, 0);
    assert_eq!(0, id_0_4_0.canonical.z);
    assert_eq!(0, id_0_4_0.canonical.x);
    assert_eq!(0, id_0_4_0.canonical.y);
    assert_eq!(4, id_0_4_0.wrap);

    let id_0_n1_0 = u(0, -1, 0);
    assert_eq!(0, id_0_n1_0.canonical.z);
    assert_eq!(0, id_0_n1_0.canonical.x);
    assert_eq!(0, id_0_n1_0.canonical.y);
    assert_eq!(-1, id_0_n1_0.wrap);

    let id_0_0_1 = u(0, 0, 1);
    assert_eq!(0, id_0_0_1.canonical.z);
    assert_eq!(0, id_0_0_1.canonical.x);
    assert_eq!(0, id_0_0_1.canonical.y);
    assert_eq!(0, id_0_0_1.wrap);

    let id_0_0_n1 = u(0, 0, -1);
    assert_eq!(0, id_0_0_n1.canonical.z);
    assert_eq!(0, id_0_0_n1.canonical.x);
    assert_eq!(0, id_0_0_n1.canonical.y);
    assert_eq!(0, id_0_0_n1.wrap);

    let id_18_262143_0 = u(18, 262_143, 0);
    assert_eq!(18, id_18_262143_0.canonical.z);
    assert_eq!(262_143, id_18_262143_0.canonical.x);
    assert_eq!(0, id_18_262143_0.canonical.y);
    assert_eq!(0, id_18_262143_0.wrap);

    let id_18_262144_0 = u(18, 262_144, 0);
    assert_eq!(18, id_18_262144_0.canonical.z);
    assert_eq!(0, id_18_262144_0.canonical.x);
    assert_eq!(0, id_18_262144_0.canonical.y);
    assert_eq!(1, id_18_262144_0.wrap);

    let a = u(2, 4, 3);
    let b = a;
    assert_eq!(2, b.canonical.z);
    assert_eq!(0, b.canonical.x);
    assert_eq!(3, b.canonical.y);
    assert_eq!(1, b.wrap);

    assert!(u(0, 0, 0) < u(1, 0, 0));
    assert!(u(1, 0, 0) < u(0, 1, 0));
    assert!(u(0, 1, 0) >= u(1, 0, 0));
    assert!(u(5, 3, 6) >= u(5, 3, 6));

    assert_eq!(ow(4, 0, c(4, 2, 3)), u(4, 2, 3).overscale_to(4));
    assert_eq!(ow(5, 0, c(4, 2, 3)), u(4, 2, 3).overscale_to(5));
    assert_eq!(ow(6, 0, c(4, 2, 3)), u(4, 2, 3).overscale_to(6));
    assert_eq!(ow(32, 0, c(4, 2, 3)), u(4, 2, 3).overscale_to(32));

    assert_eq!(uw(-1, c(1, 0, 0)), u(1, -2, 0));
    assert_eq!(uw(-1, c(1, 0, 1)), u(1, -2, 1));
    assert_eq!(uw(-1, c(1, 1, 0)), u(1, -1, 0));
    assert_eq!(uw(-1, c(1, 1, 1)), u(1, -1, 1));
    assert_eq!(uw(-1, c(0, 0, 0)), u(0, -1, 0));

    assert_eq!([u(1, 0, 0), u(1, 0, 1), u(1, 1, 0), u(1, 1, 1)], u(0, 0, 0).children());
    assert_eq!([u(1, -2, 0), u(1, -2, 1), u(1, -1, 0), u(1, -1, 1)], u(0, -1, 0).children());
}

// Not in the C++ test file: the `util::toString` / `operator<<` formats from
// src/mln/tile/tile_id_io.cpp.
#[test]
fn to_string() {
    assert_eq!("4/2/3=>6", ow(6, 0, c(4, 2, 3)).to_string());
    assert_eq!("4/2/3+1", uw(1, c(4, 2, 3)).to_string());
    assert_eq!("4/2/3-1", uw(-1, c(4, 2, 3)).to_string());
    assert_eq!("4/2/3+0", uw(0, c(4, 2, 3)).to_string());
}
