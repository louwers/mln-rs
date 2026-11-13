use geometry::Point;

/// `perp(a)` for `Point<double>`.
#[must_use]
pub fn perp(a: Point<f64>) -> Point<f64> {
    Point::new(-a.y, a.x)
}

/// The squared distance between `a` and `b`. The differences and the sum are
/// computed in `i32` (wrapping), then converted.
#[must_use]
pub fn dist_sqr_i16(a: Point<i16>, b: Point<i16>) -> f32 {
    let dx = i32::from(b.x) - i32::from(a.x);
    let dy = i32::from(b.y) - i32::from(a.y);
    dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy)) as f32
}

/// The squared distance between `a` and `b`, in float.
#[must_use]
pub fn dist_sqr_i16_f32(a: Point<i16>, b: Point<f32>) -> f32 {
    let dx = b.x - f32::from(a.x);
    let dy = b.y - f32::from(a.y);
    dx * dx + dy * dy
}

/// The distance between `a` and `b`.
#[must_use]
pub fn dist_f64(a: Point<f64>, b: Point<f64>) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

/// The unit vector from `a` to `b` (NaN when they are equal).
#[must_use]
pub fn normal(a: Point<f64>, b: Point<f64>) -> Point<f64> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let c = (dx * dx + dy * dy).sqrt();
    Point::new(dx / c, dy / c)
}

#[must_use]
pub fn cross_product(a: Point<f64>, b: Point<f64>) -> f64 {
    a.x * b.y - a.y * b.x
}

/// The signed angle from `a` to `b`.
#[must_use]
pub fn angle_between(a: Point<f64>, b: Point<f64>) -> f64 {
    (a.x * b.y - a.y * b.x).atan2(a.x * b.x + a.y * b.y)
}

#[must_use]
pub fn length_f32(a: f32, b: f32) -> f32 {
    (a * a + b * b).sqrt()
}

/// The length of `a`.
#[must_use]
pub fn mag(a: Point<f64>) -> f64 {
    (a.x * a.x + a.y * a.y).sqrt()
}

/// `a` scaled to length 1; a zero vector stays zero.
#[must_use]
pub fn unit(a: Point<f64>) -> Point<f64> {
    let magnitude = mag(a);
    if magnitude == 0.0 {
        return a;
    }
    let s = 1.0 / magnitude;
    Point::new(a.x * s, a.y * s)
}

/// Rotates `a` by `angle` radians, computed in float and truncated back.
#[must_use]
pub fn rotate_i16(a: Point<i16>, angle: f32) -> Point<i16> {
    let cos = angle.cos();
    let sin = angle.sin();
    let x = cos * f32::from(a.x) - sin * f32::from(a.y);
    let y = sin * f32::from(a.x) + cos * f32::from(a.y);
    Point::new(x as i16, y as i16)
}

#[must_use]
pub fn rotate_f32(a: Point<f32>, angle: f32) -> Point<f32> {
    let cos = angle.cos();
    let sin = angle.sin();
    Point::new(cos * a.x - sin * a.y, sin * a.x + cos * a.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(perp(Point::new(1.0, 2.0)), Point::new(-2.0, 1.0));
        assert_eq!(dist_sqr_i16(Point::new(0, 0), Point::new(3, 4)), 25.0);
        assert_eq!(unit(Point::new(0.0, 0.0)), Point::new(0.0, 0.0));
        assert_eq!(unit(Point::new(0.0, 2.0)), Point::new(0.0, 1.0));
        assert_eq!(rotate_i16(Point::new(10, 0), std::f32::consts::FRAC_PI_2), Point::new(0, 10));
        assert_eq!(length_f32(3.0, 4.0), 5.0);
    }
}
