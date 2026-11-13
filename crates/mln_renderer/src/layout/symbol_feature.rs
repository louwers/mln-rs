// C++ keeps the tile's `GeometryTileFeature` (which may borrow the source
// layer the SymbolLayout holds). A Rust feature borrows its layer, and the
// layout lives across worker messages, so a SymbolFeature keeps an owned
// copy of what the layout reads: type, id, properties and the geometry
// (C++ also clones the geometry, for mergeLines).

use feature::{Identifier, PropertyMap, Value};
use mln_expression::image::Image;
use mln_text::TaggedString;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileFeature};
use mln_util::geometry::FeatureType;

#[derive(Clone, Debug)]
pub struct SymbolFeature {
    feature_type: FeatureType,
    id: Identifier,
    properties: PropertyMap,
    /// The feature's own geometry (`feature->getGeometries()`).
    original_geometry: GeometryCollection,
    /// The mutable copy (`geometry`), merged by mergeLines and cleared once
    /// laid out.
    pub geometry: GeometryCollection,
    pub formatted_text: Option<TaggedString>,
    pub icon: Option<Image>,
    pub sort_key: f32,
    pub index: usize,
}

impl SymbolFeature {
    pub fn new(feature: &dyn GeometryTileFeature) -> Self {
        let geometry = feature.get_geometries().clone();
        SymbolFeature {
            feature_type: feature.get_type(),
            id: feature.get_id(),
            properties: feature.get_properties().clone(),
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

    fn get_properties(&self) -> &PropertyMap {
        &self.properties
    }

    fn get_id(&self) -> Identifier {
        self.id.clone()
    }

    fn get_geometries(&self) -> &GeometryCollection {
        &self.original_geometry
    }
}

/// `operator<` (by sort key), for the sorted insert in `SymbolLayout`.
#[must_use]
pub fn symbol_feature_less(lhs: &SymbolFeature, rhs: &SymbolFeature) -> bool {
    lhs.sort_key < rhs.sort_key
}
