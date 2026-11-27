// A tile's features borrow their layer, and a symbol layout lives across
// worker messages while it waits for glyphs and images, so a SymbolFeature
// keeps an owned copy of what the layout reads: type, id, properties and
// geometry, which line merging changes.

use std::borrow::Cow;

use mln_style::expression::image::Image;
use mln_style::feature::{GeometryCollection, GeometryTileFeature, Identifier, PropertyMap, Value};
use mln_style::geometry::FeatureType;

use crate::text::TaggedString;

#[derive(Clone, Debug)]
pub(crate) struct SymbolFeature {
    feature_type: FeatureType,
    id: Identifier,
    properties: PropertyMap,
    /// The feature's own geometry (`feature->getGeometries()`).
    original_geometry: GeometryCollection,
    /// The mutable copy (`geometry`), merged by mergeLines and cleared once
    /// laid out.
    pub(crate) geometry: GeometryCollection,
    pub(crate) formatted_text: Option<TaggedString>,
    pub(crate) icon: Option<Image>,
    pub(crate) sort_key: f32,
    pub(crate) index: usize,
}

impl SymbolFeature {
    /// A copy of `feature`, whose geometry is `geometry`.
    pub(crate) fn new(feature: &dyn GeometryTileFeature, geometry: GeometryCollection) -> Self {
        SymbolFeature {
            feature_type: feature.get_type(),
            id: feature.get_id(),
            properties: feature.get_properties().into_owned(),
            original_geometry: geometry.clone(),
            geometry,
            formatted_text: None,
            icon: None,
            sort_key: 0.0,
            index: 0,
        }
    }
}

impl GeometryTileFeature for SymbolFeature {
    fn get_type(&self) -> FeatureType {
        self.feature_type
    }

    fn get_value(&self, key: &str) -> Option<Value> {
        self.properties.get(key).cloned()
    }

    fn get_properties(&self) -> Cow<'_, PropertyMap> {
        Cow::Borrowed(&self.properties)
    }

    fn get_id(&self) -> Identifier {
        self.id.clone()
    }

    fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, mln_style::error::Error> {
        Ok(Cow::Borrowed(&self.original_geometry))
    }
}
