// Ports test/util/position.test.cpp.

use mln_style_types::Position;

use crate::assert_near;

#[track_caller]
fn expect_array_eq(got: [f32; 3], expected: [f32; 3]) {
    for i in 0..3 {
        assert_near(f64::from(got[i]), f64::from(expected[i]), 0.00001);
    }
}

#[track_caller]
fn expect_array_ne(got: [f32; 3], expected: [f32; 3]) {
    let eq = (0..3).filter(|&i| got[i] == expected[i]).count();
    assert_ne!(eq, 3);
}

fn create_position(pos: [f32; 3]) -> Position {
    Position::new(pos)
}

// gtest: Position.Calculations
#[test]
fn calculations() {
    let spherical = [2.0, 10.0, 270.0];
    let mut position = Position::new(spherical);

    expect_array_ne(position.get_cartesian(), spherical);
    expect_array_eq(position.get_spherical(), spherical);
    #[allow(clippy::excessive_precision)]
    expect_array_eq(
        position.get_cartesian(),
        [0.347_296_386_957_168_58, -1.969_615_459_442_138_7, 2.384_976_127_700_611e-08],
    );

    expect_array_ne(
        create_position([2.0, 30.0, 10.0]).get_spherical(),
        create_position([2.0, 30.0, 370.0]).get_spherical(),
    );
    expect_array_eq(
        create_position([2.0, 30.0, 10.0]).get_cartesian(),
        create_position([2.0, 30.0, 370.0]).get_cartesian(),
    );

    let new_spherical = [1.0, 80.0, 270.0];
    position.set(new_spherical);
    expect_array_ne(position.get_spherical(), spherical);
    #[allow(clippy::excessive_precision)]
    expect_array_ne(
        position.get_cartesian(),
        [0.347_296_386_957_168_58, -1.969_615_459_442_138_7, 2.384_976_127_700_611e-08],
    );
    #[allow(clippy::excessive_precision)]
    expect_array_eq(
        position.get_cartesian(),
        [0.984_807_729_721_069_34, -0.173_648_297_786_712_65, 1.192_488_063_850_305_5e-08],
    );
}
