use std::borrow::Cow;
use std::ops::{Deref, DerefMut, Range};
use std::sync::Arc;

use geometry::Point;

use crate::error::Error;
use crate::feature::{Identifier, PropertyMap, Value};
use crate::geometry::FeatureType;

/// Normalized vector tile coordinates. Each geometry coordinate represents a
/// point in a bidimensional space, varying from -V...0...+V, where V is the
/// maximum extent applicable.
pub type GeometryCoordinate = Point<i16>;

pub type GeometryCoordinates = Vec<GeometryCoordinate>;

/// The rings or lines of a feature's geometry, with the triangles of a
/// pre-tessellated polygon.
///
/// `Clone` keeps the triangles, as the C++ `clone()` does (C++ makes the
/// copy constructor private). `==` compares only the rings, as the
/// `std::vector` base does in C++.
#[derive(Clone, Debug, Default)]
pub struct GeometryCollection {
    rings: Vec<GeometryCoordinates>,
    /// The pre-tessellated triangles of an MLT polygon feature: C++
    /// `std::shared_ptr<const mlt::MapLibreTile> triangleOwner` plus the
    /// `std::span<const std::uint32_t> triangles` into its index buffer; here
    /// the shared buffer and the span's range in it.
    triangles: Option<(Arc<[u32]>, Range<usize>)>,
}

impl GeometryCollection {
    #[must_use]
    pub const fn new() -> Self {
        GeometryCollection { rings: Vec::new(), triangles: None }
    }

    /// The triangles of a pre-tessellated polygon: three indices per
    /// triangle into the vertices of the rings, numbered from 0 across the
    /// rings in order. Empty unless the tile carried triangles. Nothing
    /// checks that the indices are in range.
    #[must_use]
    pub fn get_triangles(&self) -> &[u32] {
        match &self.triangles {
            Some((owner, range)) => &owner[range.clone()],
            None => &[],
        }
    }

    /// Sets the triangles to `owner[range]`. Panics if `range` is out of
    /// bounds.
    pub fn set_triangles(&mut self, owner: Arc<[u32]>, range: Range<usize>) {
        assert!(range.start <= range.end && range.end <= owner.len());
        self.triangles = Some((owner, range));
    }
}

impl PartialEq for GeometryCollection {
    fn eq(&self, other: &Self) -> bool {
        self.rings == other.rings
    }
}

impl From<Vec<GeometryCoordinates>> for GeometryCollection {
    fn from(rings: Vec<GeometryCoordinates>) -> Self {
        GeometryCollection { rings, triangles: None }
    }
}

impl Deref for GeometryCollection {
    type Target = Vec<GeometryCoordinates>;
    fn deref(&self) -> &Self::Target {
        &self.rings
    }
}

impl DerefMut for GeometryCollection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rings
    }
}

static EMPTY_PROPERTIES: PropertyMap = PropertyMap::new();
static EMPTY_GEOMETRIES: GeometryCollection = GeometryCollection::new();

/// A feature as expressions and buckets read it: a view of a tile's
/// feature (`mln_engine::tile::Feature`), or a feature made for evaluating an
/// expression. Reading a value never fails: a reader whose data can be
/// malformed checks it when it makes the view.
pub trait GeometryTileFeature {
    fn get_type(&self) -> FeatureType;

    /// The value of the property `key`; `None` for none or a null value.
    fn get_value(&self, key: &str) -> Option<Value>;

    /// The feature's properties; empty by default. A view that decodes its
    /// properties on demand returns them owned.
    fn get_properties(&self) -> Cow<'_, PropertyMap> {
        Cow::Borrowed(&EMPTY_PROPERTIES)
    }

    /// The feature's id; [`Identifier::Null`] by default.
    fn get_id(&self) -> Identifier {
        Identifier::Null
    }

    /// The feature's geometries; empty by default. A view that decodes its
    /// geometry on demand returns it owned, and fails for a geometry it
    /// can't decode.
    fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, Error> {
        Ok(Cow::Borrowed(&EMPTY_GEOMETRIES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Feature;

    impl GeometryTileFeature for Feature {
        fn get_type(&self) -> FeatureType {
            FeatureType::Point
        }
        fn get_value(&self, _key: &str) -> Option<Value> {
            None
        }
    }

    #[test]
    fn defaults() {
        let f = Feature;
        assert!(f.get_properties().is_empty());
        assert_eq!(f.get_id(), Identifier::Null);
        assert!(f.get_geometries().unwrap().is_empty());
        let mut c = GeometryCollection::new();
        c.push(vec![Point::new(1, 2)]);
        assert_eq!(c.clone()[0][0], Point::new(1, 2));
    }

    #[test]
    fn triangles() {
        let mut c = GeometryCollection::from(vec![vec![Point::new(0, 0), Point::new(1, 0), Point::new(0, 1)]]);
        assert_eq!(c.get_triangles(), <&[u32]>::default());
        let owner: Arc<[u32]> = Arc::from(vec![9, 9, 0, 1, 2, 9]);
        c.set_triangles(owner, 2..5);
        assert_eq!(c.get_triangles(), &[0, 1, 2]);
        // The copy keeps them; == ignores them, as the C++ vector base does.
        assert_eq!(c.clone().get_triangles(), &[0, 1, 2]);
        assert_eq!(c, GeometryCollection::from(c.to_vec()));
    }
}
