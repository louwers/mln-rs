//! The feature queries of a render session.

use mln_util::error::Error;
use mln_util::geo::ScreenCoordinate;

/// The screen area that a rendered feature query covers, in logical pixels
/// from the top left corner.
#[derive(Clone, Debug, PartialEq)]
pub enum RenderedQueryGeometry {
    Point(ScreenCoordinate),
    /// The rectangle from its `min` corner to its `max` corner.
    Box {
        min: ScreenCoordinate,
        max: ScreenCoordinate,
    },
    /// A line through its points.
    LineString(Vec<ScreenCoordinate>),
}

impl RenderedQueryGeometry {
    /// The geometry's points, or why the query can't use it.
    pub(crate) fn points(&self) -> Result<Vec<ScreenCoordinate>, Error> {
        let points = match self {
            RenderedQueryGeometry::Point(point) => vec![*point],
            RenderedQueryGeometry::Box { min, max } => {
                vec![*min, ScreenCoordinate::new(max.x, min.y), *max, ScreenCoordinate::new(min.x, max.y), *min]
            }
            RenderedQueryGeometry::LineString(points) => {
                if points.is_empty() {
                    return Err(Error::invalid_argument("a query line string needs points"));
                }
                points.clone()
            }
        };
        if points.iter().any(|point| !point.x.is_finite() || !point.y.is_finite()) {
            return Err(Error::invalid_argument("a query point must be finite"));
        }
        Ok(points)
    }
}
