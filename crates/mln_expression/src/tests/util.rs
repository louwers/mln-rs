use crate::util::rgba;

// gtest: Expression.Util_rgba
#[test]
fn util_rgba() {
    let valid = rgba(0.0, 0.0, 0.0, 0.0);
    let valid = valid.expect("valid");
    assert_eq!(valid.r, 0.0);
    assert_eq!(valid.g, 0.0);
    assert_eq!(valid.b, 0.0);
    assert_eq!(valid.a, 0.0);

    assert!(rgba(0.0, 0.0, 0.0, -0.1).is_err());
    assert!(rgba(0.0, 0.0, 0.0, 1.1).is_err());
    assert!(rgba(0.0, 0.0, -1.0, 1.0).is_err());
    assert!(rgba(0.0, 0.0, 256.0, 1.0).is_err());
    assert!(rgba(0.0, -1.0, 0.0, 1.0).is_err());
    assert!(rgba(0.0, 256.0, 0.0, 1.0).is_err());
    assert!(rgba(-1.0, 1.0, 0.0, 1.0).is_err());
    assert!(rgba(-256.0, 1.0, 0.0, 1.0).is_err());
}
