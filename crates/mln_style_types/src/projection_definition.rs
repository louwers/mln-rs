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

#[must_use]
pub(crate) fn projection_type_name(type_: ProjectionType) -> &'static str {
    match type_ {
        ProjectionType::Mercator => MERCATOR_NAME,
        ProjectionType::VerticalPerspective => VERTICAL_PERSPECTIVE_NAME,
        ProjectionType::Globe => GLOBE_NAME,
    }
}

#[must_use]
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
    #[must_use]
    pub fn new(type_: ProjectionType) -> Self {
        ProjectionDefinition { from: type_, to: type_, transition: 1.0 }
    }

    #[must_use]
    pub fn between(from: ProjectionType, to: ProjectionType, transition: f64) -> Self {
        ProjectionDefinition { from, to, transition }
    }

    /// The single projection named `name`. Debug builds assert that the name
    /// is one of `projection_type_name`'s; release builds fall back to
    /// Mercator.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        ProjectionDefinition::new(type_from_name_or_mercator(name))
    }

    #[must_use]
    pub fn between_names(from: &str, to: &str, transition: f64) -> Self {
        ProjectionDefinition::between(type_from_name_or_mercator(from), type_from_name_or_mercator(to), transition)
    }

    /// The definition that draws this one at `zoom`, with `globe` resolved:
    /// `globe` is the vertical perspective up to zoom 11 and Mercator from
    /// zoom 12, blended in between, at either end of a definition, and each
    /// end counts with its share of the globe at the zoom.
    #[must_use]
    pub fn at_zoom(&self, zoom: f64) -> ProjectionDefinition {
        const GLOBE_TO_MERCATOR_START_ZOOM: f64 = 11.0;
        const GLOBE_TO_MERCATOR_END_ZOOM: f64 = 12.0;
        if self.from != ProjectionType::Globe && self.to != ProjectionType::Globe {
            return *self;
        }
        let globe_share = 1.0
            - ((zoom - GLOBE_TO_MERCATOR_START_ZOOM) / (GLOBE_TO_MERCATOR_END_ZOOM - GLOBE_TO_MERCATOR_START_ZOOM))
                .clamp(0.0, 1.0);
        let share = |end: ProjectionType| match end {
            ProjectionType::Globe => globe_share,
            ProjectionType::Mercator => 0.0,
            ProjectionType::VerticalPerspective => 1.0,
        };
        let globe = if self.from == self.to {
            share(self.from)
        } else {
            interpolate(&share(self.from), &share(self.to), self.transition.clamp(0.0, 1.0))
        };
        if globe >= 1.0 {
            return ProjectionDefinition::new(ProjectionType::VerticalPerspective);
        }
        if globe <= 0.0 {
            return ProjectionDefinition::new(ProjectionType::Mercator);
        }
        ProjectionDefinition::between(ProjectionType::VerticalPerspective, ProjectionType::Mercator, 1.0 - globe)
    }

    /// How far the definition is toward the globe: 0 is Mercator, 1 the globe
    /// (GL JS `GlobeProjection.transitionState`).
    #[must_use]
    pub fn transition_state(&self) -> f64 {
        let is_globe = |type_: ProjectionType| type_ != ProjectionType::Mercator;
        if self.from == self.to {
            return if is_globe(self.from) { 1.0 } else { 0.0 };
        }
        // A transition past either end, from a literal or an overshooting
        // curve, holds at that end's projection.
        let share = self.transition.clamp(0.0, 1.0);
        if is_globe(self.from) && !is_globe(self.to) {
            return 1.0 - share;
        }
        if !is_globe(self.from) && is_globe(self.to) {
            return share;
        }
        1.0
    }

    #[must_use]
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
