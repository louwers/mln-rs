use mln_style_types::Rotation;
use mln_util::interpolate::Interpolate;

use crate::assert_near;

// gtest: Rotation.Calculations
#[test]
fn calculations() {
    let rot = Rotation::new(42.0);
    assert_eq!(rot, Rotation::new(42.0 + 360.0));
    let rot2 = Rotation::new(-42.0);
    assert_eq!(rot2, Rotation::new(360.0 - 42.0));

    // mln::util::Interpolator<mln::style::Rotation> i; i(rot, rot2, 0.5)
    assert_near(rot.interpolate(&rot2, 0.5).get_angle(), 0.0, 0.00001);
}
