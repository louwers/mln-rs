//! The parts a layer shows, from the bottom up, and where each lies on the
//! ground around an anchor of its position.

use mln_engine::camera::View;
use mln_style::color::Color;

use crate::anchor::Anchor;
use crate::paint;
use crate::program::PartDraw;

/// A part's shape, as the program draws it in its unit square.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// A disc of radius 1 with a border inside its edge.
    Disc = 0,
    /// A sector of the unit disc about north, which fades outward.
    Sector = 1,
    /// A soft disc.
    Shadow = 2,
    /// An arrow pointing north.
    Arrow = 3,
}

/// Where a part's center is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// At the position.
    Ground,
    /// Lifted up the screen under pitch.
    Lifted,
    /// Dropped down the screen under pitch.
    Dropped,
}

/// A part's radius.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Radius {
    /// Logical pixels, scaled by the perspective compensation.
    Pixels(f32),
    /// Meters on the ground.
    Meters(f32),
}

/// A disc's border width.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Border {
    /// Logical pixels.
    Pixels(f32),
    /// A share of the disc's radius.
    Share(f32),
}

impl Border {
    /// Whether the border has any width.
    fn is_positive(self) -> bool {
        match self {
            Border::Pixels(width) | Border::Share(width) => width > 0.0,
        }
    }
}

/// One of the shapes a layer draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Part {
    pub(crate) shape: Shape,
    place: Place,
    radius: Radius,
    /// Whether the part turns with the bearing.
    turns: bool,
    border_width: Border,
    /// A sector's half width, in radians.
    half_width: f32,
    fill: Color,
    border: Color,
    /// Whether a query hits the part.
    pub(crate) hit: bool,
}

impl Part {
    /// Where a query hits the part, in its unit square: the arrow's
    /// triangle, or a polygon inside the disc.
    pub(crate) fn outline(&self) -> Vec<[f64; 2]> {
        if self.shape == Shape::Arrow {
            return vec![[0.0, -1.0], [0.39, -0.28], [-0.39, -0.28]];
        }
        let sides = 24;
        let corner = |i: u32| (f64::from(i) * std::f64::consts::TAU / f64::from(sides)).sin_cos();
        (0..sides).map(corner).map(|(sin, cos)| [cos, sin]).collect()
    }
}

/// The parts that `paint` shows, from the bottom up: those with a size and
/// a color.
pub(crate) fn parts(paint: &paint::PossiblyEvaluated) -> impl Iterator<Item = Part> {
    let value = |value: f32| value.max(0.0);
    let bearing_visible = *paint.puck_bearing_visible.uniform();
    let part = |shape, place, radius, fill| Part {
        shape,
        place,
        radius,
        turns: true,
        border_width: Border::Pixels(0.0),
        half_width: 0.0,
        fill,
        border: Color::new(0.0, 0.0, 0.0, 0.0),
        hit: false,
    };

    let accuracy = Part {
        turns: false,
        border_width: Border::Pixels(value(*paint.puck_accuracy_border_width.uniform())),
        border: *paint.puck_accuracy_border_color.uniform(),
        ..part(
            Shape::Disc,
            Place::Ground,
            Radius::Meters(value(*paint.puck_accuracy_radius.uniform())),
            *paint.puck_accuracy_color.uniform(),
        )
    };
    let half_width = paint.puck_bearing_accuracy.uniform().clamp(0.0, 180.0).to_radians();
    let sector = Part {
        half_width,
        ..part(
            Shape::Sector,
            Place::Ground,
            Radius::Pixels(if half_width > 0.0 { value(*paint.puck_bearing_accuracy_radius.uniform()) } else { 0.0 }),
            *paint.puck_bearing_accuracy_color.uniform(),
        )
    };
    let shadow = part(
        Shape::Shadow,
        Place::Dropped,
        Radius::Pixels(value(*paint.puck_shadow_radius.uniform())),
        *paint.puck_shadow_color.uniform(),
    );
    let arrow = Part {
        hit: true,
        ..part(
            Shape::Arrow,
            Place::Lifted,
            Radius::Pixels(value(*paint.puck_bearing_radius.uniform())),
            *paint.puck_bearing_arrow_color.uniform(),
        )
    };
    // The border lies outside the fill.
    let (radius, border) = (value(*paint.puck_radius.uniform()), value(*paint.puck_border_width.uniform()));
    let outer = if radius > 0.0 { radius + border } else { 0.0 };
    let puck = Part {
        border_width: Border::Share(if outer > 0.0 { border / outer } else { 0.0 }),
        border: *paint.puck_border_color.uniform(),
        hit: true,
        ..part(Shape::Disc, Place::Lifted, Radius::Pixels(outer), *paint.puck_color.uniform())
    };

    let heading = [sector, arrow].map(|part| bearing_visible.then_some(part));
    [Some(accuracy), heading[0], Some(shadow), heading[1], Some(puck)]
        .into_iter()
        .flatten()
        .filter(|part| part.fill.a > 0.0 || (part.border.a > 0.0 && part.border_width.is_positive()))
        .filter(|part| match part.radius {
            Radius::Pixels(radius) | Radius::Meters(radius) => radius > 0.0,
        })
}

/// The ground around an anchor of a layer's position, as a view sees it:
/// how large the parts are there and where they move under pitch, in the
/// anchor tile's units.
pub(crate) struct Ground {
    point: [f64; 2],
    height: f64,
    /// The tile units of a logical pixel of a part's radius.
    pixel: f64,
    meter: f64,
    /// How far a lifted part moves.
    lift: [f64; 2],
    /// The bearing's sine and cosine.
    heading: (f64, f64),
}

impl Ground {
    /// The ground at `anchor`, of a layer of `paint` seen by `view`.
    pub(crate) fn new(paint: &paint::PossiblyEvaluated, view: &View, anchor: &Anchor) -> Ground {
        // How much farther the anchor is than the view's center. Perspective
        // shrinks a part on the screen by as much, which its size in tile
        // units makes up for as far as the compensation says: from 0, a size
        // on the ground, to 1, a size on the screen.
        let farther = anchor.distance / view.camera_to_center_distance();
        let compensation = f64::from(paint.puck_perspective_compensation.uniform().clamp(0.0, 1.0));
        let scale = 1.0 - compensation + farther * compensation;
        let pixel = scale * anchor.pixels_to_tile_units;

        // Up the screen is the camera's bearing on the ground.
        let up = -view.angle();
        let lift = view.pitch() * f64::from(paint.puck_tilt_displacement.uniform().max(0.0)) * farther * pixel;
        Ground {
            point: anchor.point,
            height: anchor.height,
            pixel,
            meter: anchor.tile_units_per_meter,
            lift: [up.sin() * lift, -up.cos() * lift],
            heading: paint.puck_bearing.uniform().get_angle().to_radians().sin_cos(),
        }
    }

    /// Where `part`'s unit square lies: its center, and its x and y axes.
    pub(crate) fn square(&self, part: &Part) -> [[f64; 2]; 3] {
        let [x, y] = self.point;
        let [dx, dy] = self.lift;
        let center = match part.place {
            Place::Ground => [x, y],
            Place::Lifted => [x + dx, y + dy],
            Place::Dropped => [x - dx, y - dy],
        };
        let radius = match part.radius {
            Radius::Pixels(radius) => f64::from(radius) * self.pixel,
            Radius::Meters(radius) => f64::from(radius) * self.meter,
        };
        // Turned clockwise on the ground, whose y goes south.
        let (sin, cos) = if part.turns { self.heading } else { (0.0, 1.0) };
        [center, [cos * radius, sin * radius], [-sin * radius, cos * radius]]
    }

    /// The draw of `part`, at `pixel_ratio` device pixels a logical pixel.
    pub(crate) fn draw(&self, part: &Part, pixel_ratio: f32) -> PartDraw {
        let [center, axis_x, axis_y] = self.square(part).map(|v| v.map(|v| v as f32));
        let (border_width, share) = match part.border_width {
            Border::Pixels(width) => (width * pixel_ratio, 0.0),
            Border::Share(share) => (share, 1.0),
        };
        let [x, y] = self.point.map(|v| v as f32);
        PartDraw {
            style: [f32::from(part.shape as u8), border_width, part.half_width, share],
            fill: part.fill.to_float_array(),
            border: part.border.to_float_array(),
            ground: [x, y, self.height as f32, 0.0],
            center,
            axis_x,
            axis_y,
            pad1: 0.0,
            pad2: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use mln_style::property::evaluated::Evaluated;

    use super::*;

    fn shapes(paint: &paint::PossiblyEvaluated) -> Vec<(Shape, bool)> {
        parts(paint).map(|part| (part.shape, part.hit)).collect()
    }

    #[test]
    fn the_parts_draw_from_the_bottom_up_and_only_with_a_size() {
        let mut paint = paint::PossiblyEvaluated {
            puck_radius: Evaluated::Constant(8.0),
            puck_color: Evaluated::Constant(Color::black()),
            puck_bearing_radius: Evaluated::Constant(18.0),
            puck_bearing_arrow_color: Evaluated::Constant(Color::white()),
            ..Default::default()
        };
        assert_eq!(vec![(Shape::Disc, true)], shapes(&paint));

        paint.puck_bearing_visible = Evaluated::Constant(true);
        paint.puck_accuracy_radius = Evaluated::Constant(10.0);
        paint.puck_accuracy_color = Evaluated::Constant(Color::black());
        assert_eq!(vec![(Shape::Disc, false), (Shape::Arrow, true), (Shape::Disc, true)], shapes(&paint));

        // A sector shows with a width, and a shadow with a radius.
        paint.puck_bearing_accuracy_radius = Evaluated::Constant(30.0);
        paint.puck_bearing_accuracy_color = Evaluated::Constant(Color::black());
        paint.puck_shadow_radius = Evaluated::Constant(4.0);
        paint.puck_shadow_color = Evaluated::Constant(Color::black());
        assert_eq!(4, shapes(&paint).len());
        paint.puck_bearing_accuracy = Evaluated::Constant(20.0);
        let expected = [Shape::Disc, Shape::Sector, Shape::Shadow, Shape::Arrow, Shape::Disc];
        assert_eq!(expected.to_vec(), shapes(&paint).into_iter().map(|(shape, _)| shape).collect::<Vec<_>>());

        // A transparent puck shows only with a border that has a width.
        paint.puck_color = Evaluated::Constant(Color::new(0.0, 0.0, 0.0, 0.0));
        paint.puck_border_color = Evaluated::Constant(Color::black());
        paint.puck_border_width = Evaluated::Constant(0.0);
        assert_eq!(Some(&(Shape::Arrow, true)), shapes(&paint).last());
        paint.puck_border_width = Evaluated::Constant(2.0);
        assert_eq!(Some(&(Shape::Disc, true)), shapes(&paint).last());

        paint.puck_radius = Evaluated::Constant(0.0);
        assert_eq!(Some(&(Shape::Arrow, true)), shapes(&paint).last());
    }
}
