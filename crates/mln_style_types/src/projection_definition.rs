// Ports include/mln/style/projection_definition.hpp +
// src/mln/style/projection_definition.cpp, plus the
// `util::Interpolator<ProjectionDefinition>` specialization from
// include/mln/util/interpolate.hpp.

use feature::Value;
use mln_util::interpolate::{Interpolate, interpolate};

/// The names the style's `projection.type` accepts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ProjectionType {
    #[default]
    Mercator,
    VerticalPerspective,
    /// The vertical perspective that hands over to Mercator between zoom 11 and 12.
    Globe,
}

const MERCATOR_NAME: &str = "mercator";
const VERTICAL_PERSPECTIVE_NAME: &str = "vertical-perspective";
const GLOBE_NAME: &str = "globe";

/// `projectionTypeName`.
pub fn projection_type_name(type_: ProjectionType) -> &'static str {
    match type_ {
        ProjectionType::Mercator => MERCATOR_NAME,
        ProjectionType::VerticalPerspective => VERTICAL_PERSPECTIVE_NAME,
        ProjectionType::Globe => GLOBE_NAME,
    }
}

/// `projectionTypeFromName`.
pub fn projection_type_from_name(name: &str) -> Option<ProjectionType> {
    match name {
        MERCATOR_NAME => Some(ProjectionType::Mercator),
        VERTICAL_PERSPECTIVE_NAME => Some(ProjectionType::VerticalPerspective),
        GLOBE_NAME => Some(ProjectionType::Globe),
        _ => None,
    }
}

fn type_from_name_or_mercator(name: &str) -> ProjectionType {
    let type_ = projection_type_from_name(name);
    debug_assert!(type_.is_some(), "unknown projection name {name}");
    type_.unwrap_or(ProjectionType::Mercator)
}

/// `std::clamp(x, 0.0, 1.0)`: `x < lo ? lo : hi < x ? hi : x` (NaN and -0.0
/// pass through).
fn clamp_unit(x: f64) -> f64 {
    if x < 0.0 {
        0.0
    } else if 1.0 < x {
        1.0
    } else {
        x
    }
}

/// The value of the style's `projection.type`: a single projection, or a
/// transition between two.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectionDefinition {
    pub from: ProjectionType,
    pub to: ProjectionType,
    pub transition: f64,
}

impl Default for ProjectionDefinition {
    fn default() -> Self {
        ProjectionDefinition { from: ProjectionType::Mercator, to: ProjectionType::Mercator, transition: 1.0 }
    }
}

impl ProjectionDefinition {
    /// `explicit ProjectionDefinition(ProjectionType)`.
    pub fn new(type_: ProjectionType) -> Self {
        ProjectionDefinition { from: type_, to: type_, transition: 1.0 }
    }

    /// `ProjectionDefinition(ProjectionType from, ProjectionType to, double transition)`.
    pub fn between(from: ProjectionType, to: ProjectionType, transition: f64) -> Self {
        ProjectionDefinition { from, to, transition }
    }

    /// `explicit ProjectionDefinition(std::string_view)`; asserts the name is
    /// one of `projection_type_name`'s.
    pub fn from_name(name: &str) -> Self {
        ProjectionDefinition::new(type_from_name_or_mercator(name))
    }

    /// `ProjectionDefinition(std::string_view from, std::string_view to, double transition)`.
    pub fn between_names(from: &str, to: &str, transition: f64) -> Self {
        ProjectionDefinition::between(type_from_name_or_mercator(from), type_from_name_or_mercator(to), transition)
    }

    /// How far the definition is toward the globe: 0 is Mercator, 1 the globe
    /// (GL JS `GlobeProjection.transitionState`).
    pub fn transition_state(&self) -> f64 {
        let is_globe = |type_: ProjectionType| type_ != ProjectionType::Mercator;
        if self.from == self.to {
            return if is_globe(self.from) { 1.0 } else { 0.0 };
        }
        // A transition past either end, from a literal or an overshooting
        // curve, holds at that end's projection.
        let share = clamp_unit(self.transition);
        if is_globe(self.from) && !is_globe(self.to) {
            return 1.0 - share;
        }
        if !is_globe(self.from) && is_globe(self.to) {
            return share;
        }
        1.0
    }

    pub fn serialize(&self) -> Value {
        if self.from == self.to && self.transition == 1.0 {
            return Value::String(projection_type_name(self.from).to_owned());
        }
        Value::Array(vec![
            Value::String(projection_type_name(self.from).to_owned()),
            Value::String(projection_type_name(self.to).to_owned()),
            Value::Double(self.transition),
        ])
    }
}

/// `Interpolator<ProjectionDefinition>`.
impl Interpolate for ProjectionDefinition {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        if self.from == self.to && to.from == to.to {
            return ProjectionDefinition::between(self.from, to.to, t);
        }
        // A stop that is already between two projections keeps its share of
        // the globe.
        ProjectionDefinition::between(
            ProjectionType::Mercator,
            ProjectionType::VerticalPerspective,
            interpolate(&self.transition_state(), &to.transition_state(), t),
        )
    }
}
