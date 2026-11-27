use std::borrow::Cow;
use std::sync::Arc;

use mln_style::feature::{
    GeometryCollection, GeometryTileFeature, Identifier, PropertyMap, Value, to_geometry_collection,
};
use mln_style::geometry::{FeatureType, to_feature_type};
use mln_wire::{Reader, Wire, Writer, payload};

use crate::tile::TileError;
use crate::tile::cut::{CutData, CutFeature, CutLayer};
use crate::tile::mlt::{MltFeature, MltLayer, MltTile};
use crate::tile::mvt::{MvtFeature, MvtLayer, MvtTile};

/// A feature in tile coordinates, as GeoJSON and custom geometry sources cut
/// them.
pub type GeoJsonFeature = mln_style::feature::Feature<i16, Arc<PropertyMap>>;

/// The data of a vector tile. Cloning is cheap: each kind shares what it is
/// made from, and what has been decoded of it.
///
/// It is written (`mln_wire::Wire`) as what it is made from: the bytes of an
/// MVT or MLT tile, as a payload (`mln_wire::payload`), or the features.
#[derive(Clone)]
pub enum TileData {
    Mvt(MvtTile),
    Mlt(MltTile),
    /// One layer, which every layer name finds.
    Features(Arc<Vec<GeoJsonFeature>>),
    /// A tile past its tileset's last zoom: the part of its ancestor's data
    /// there that reaches it.
    Cut(Arc<CutData>),
}

impl TileData {
    /// The first layer named `name`; `None` if the data has none. Fails for
    /// a tile whose layers can't be found, or a layer that can't be decoded.
    pub fn layer(&self, name: &str) -> Result<Option<TileLayer<'_>>, TileError> {
        Ok(match self {
            TileData::Mvt(tile) => tile.layer(name)?.map(TileLayer::Mvt),
            TileData::Mlt(tile) => tile.layer(name).map(TileLayer::Mlt),
            TileData::Features(features) => Some(TileLayer::Features(features)),
            TileData::Cut(data) => data.layer(name)?.map(TileLayer::Cut),
        })
    }

    /// Whether the data has a layer named `name`, found without decoding the
    /// layer where the data can. Fails as [`layer`](Self::layer) does.
    pub(crate) fn has_layer(&self, name: &str) -> Result<bool, TileError> {
        match self {
            TileData::Mvt(tile) => tile.has_layer(name),
            TileData::Mlt(tile) => Ok(tile.layer(name).is_some()),
            TileData::Features(_) => Ok(true),
            TileData::Cut(data) => data.has_layer(name),
        }
    }
}

/// A layer of a [`TileData`].
pub enum TileLayer<'a> {
    Mvt(MvtLayer<'a>),
    Mlt(MltLayer<'a>),
    Features(&'a [GeoJsonFeature]),
    /// The features of an ancestor's layer that reach a cut tile, with their
    /// geometry there.
    Cut(Arc<CutLayer>),
}

impl TileLayer<'_> {
    /// The layer's name; empty for features.
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        match self {
            TileLayer::Mvt(layer) => layer.name(),
            TileLayer::Mlt(layer) => layer.name(),
            TileLayer::Features(_) => "",
            TileLayer::Cut(layer) => layer.name(),
        }
    }

    /// The units across the tile of the layer as encoded; its features'
    /// geometry is in [`EXTENT`](mln_style::constants::EXTENT)'s.
    #[must_use]
    pub(crate) fn extent(&self) -> u32 {
        match self {
            TileLayer::Mvt(layer) => layer.extent(),
            TileLayer::Mlt(layer) => layer.extent(),
            TileLayer::Features(_) | TileLayer::Cut(_) => mln_style::constants::EXTENT as u32,
        }
    }

    /// The number of features.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            TileLayer::Mvt(layer) => layer.len(),
            TileLayer::Mlt(layer) => layer.len(),
            TileLayer::Features(features) => features.len(),
            TileLayer::Cut(layer) => layer.features().len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The feature at `index`. Fails past the end, and for a feature that
    /// can't be decoded or whose properties are malformed, so that reading
    /// the feature's values can't fail.
    pub fn feature(&self, index: usize) -> Result<Feature<'_>, TileError> {
        match self {
            TileLayer::Mvt(layer) => layer.feature(index).map(Feature::Mvt),
            TileLayer::Mlt(layer) => layer.feature(index).map(Feature::Mlt),
            TileLayer::Features(features) => match features.get(index) {
                Some(feature) => Ok(Feature::Features(feature)),
                None => Err(TileError::vector_range_check(index, features.len())),
            },
            TileLayer::Cut(layer) => match layer.features().get(index) {
                Some(feature) => Ok(Feature::Cut(feature)),
                None => Err(TileError::vector_range_check(index, layer.features().len())),
            },
        }
    }

    /// The features in groups whose members have the same geometry type and
    /// the same values under each of `keys` (or lack each alike), so that an
    /// expression reading only those of a feature gives every member the
    /// same result. `None` if the layer can't group them cheaply, or nearly
    /// every feature would be a group of its own.
    #[must_use]
    pub(crate) fn group_features(&self, keys: &[String]) -> Option<FeatureGroups> {
        match self {
            TileLayer::Mvt(layer) => layer.group_features(keys),
            TileLayer::Mlt(layer) => layer.group_features(keys),
            TileLayer::Features(_) | TileLayer::Cut(_) => None,
        }
    }
}

/// `TileLayer::group_features`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureGroups {
    /// Each feature's group.
    pub(crate) of_feature: Vec<u32>,
    /// Each group's first feature; groups are numbered in feature order.
    pub(crate) first: Vec<usize>,
}

/// A feature of a [`TileLayer`]. Expressions and buckets read it as a
/// [`GeometryTileFeature`].
pub enum Feature<'a> {
    Mvt(MvtFeature<'a>),
    Mlt(MltFeature<'a>),
    Features(&'a GeoJsonFeature),
    Cut(&'a CutFeature),
}

impl Feature<'_> {
    /// The feature's lines or rings, decoded. Fails for a geometry that
    /// can't be decoded.
    pub fn geometry(&self) -> Result<GeometryCollection, TileError> {
        match self {
            Feature::Mvt(feature) => feature.geometry(),
            Feature::Mlt(feature) => feature.geometry(),
            Feature::Features(feature) => Ok(to_geometry_collection(feature.geometry.as_ref())),
            Feature::Cut(feature) => Ok(feature.geometry().clone()),
        }
    }
}

impl GeometryTileFeature for Feature<'_> {
    fn get_type(&self) -> FeatureType {
        match self {
            Feature::Mvt(feature) => feature.get_type(),
            Feature::Mlt(feature) => feature.get_type(),
            Feature::Features(feature) => to_feature_type(feature.geometry.as_ref()),
            Feature::Cut(feature) => feature.get_type(),
        }
    }

    fn get_value(&self, key: &str) -> Option<Value> {
        match self {
            Feature::Mvt(feature) => feature.get_value(key),
            Feature::Mlt(feature) => feature.get_value(key),
            Feature::Features(feature) => feature.properties.get(key).cloned(),
            Feature::Cut(feature) => feature.get_value(key),
        }
    }

    fn get_properties(&self) -> Cow<'_, PropertyMap> {
        match self {
            Feature::Mvt(feature) => Cow::Owned(feature.get_properties()),
            Feature::Mlt(feature) => Cow::Owned(feature.get_properties()),
            Feature::Features(feature) => Cow::Borrowed(&feature.properties),
            Feature::Cut(feature) => feature.get_properties(),
        }
    }

    fn get_id(&self) -> Identifier {
        match self {
            Feature::Mvt(feature) => feature.get_id(),
            Feature::Mlt(feature) => feature.get_id(),
            Feature::Features(feature) => feature.id.clone(),
            Feature::Cut(feature) => feature.get_id(),
        }
    }

    fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, mln_style::error::Error> {
        match self {
            Feature::Cut(feature) => feature.get_geometries(),
            Feature::Mvt(_) | Feature::Mlt(_) | Feature::Features(_) => Ok(Cow::Owned(self.geometry()?)),
        }
    }
}

impl Wire for TileData {
    fn write(&self, w: &mut Writer) {
        match self {
            TileData::Mvt(tile) => {
                w.push(0);
                payload::bytes::write(tile.bytes(), w);
            }
            TileData::Mlt(tile) => {
                w.push(1);
                tile.fast_pfor_enabled().write(w);
                payload::bytes::write(tile.bytes(), w);
            }
            TileData::Features(features) => {
                w.push(2);
                features.write(w);
            }
            TileData::Cut(data) => {
                w.push(3);
                data.write(w);
            }
        }
    }

    fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(match r.byte()? {
            0 => TileData::Mvt(MvtTile::new(Arc::new(payload::bytes::read(r)?))),
            1 => {
                let fast_pfor_enabled = bool::read(r)?;
                TileData::Mlt(MltTile::new(Arc::new(payload::bytes::read(r)?), fast_pfor_enabled))
            }
            2 => TileData::Features(Arc::new(Vec::read(r)?)),
            3 => TileData::Cut(Arc::new(CutData::read(r)?)),
            _ => return r.fail_tag(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use geometry::Point;

    use super::*;

    fn fixture(name: &str) -> Arc<Vec<u8>> {
        let path = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("testdata/test/fixtures/map/issue12432")
            .join(name);
        Arc::new(std::fs::read(path).unwrap())
    }

    fn features() -> Arc<Vec<GeoJsonFeature>> {
        let point = GeoJsonFeature {
            geometry: Some(geo_types::Point::new(1, 2).into()),
            properties: Arc::new(PropertyMap::from([("k".to_owned(), Value::Bool(true))])),
            id: Identifier::Uint(7),
        };
        Arc::new(vec![point, GeoJsonFeature::default()])
    }

    #[test]
    fn features_are_one_layer_under_every_name() {
        let data = TileData::Features(features());
        assert!(data.has_layer("any").unwrap());
        let layer = data.layer("any").unwrap().unwrap();
        assert_eq!((layer.name(), layer.len()), ("", 2));
        assert!(matches!(layer.feature(2), Err(TileError::OutOfRange(_))));
        assert_eq!(layer.group_features(&["k".to_owned()]), None);

        let point = layer.feature(0).unwrap();
        assert_eq!(point.get_type(), FeatureType::Point);
        assert_eq!(point.get_id(), Identifier::Uint(7));
        assert_eq!(point.get_value("k"), Some(Value::Bool(true)));
        assert!(matches!(point.get_properties(), Cow::Borrowed(properties) if properties.len() == 1));
        assert_eq!(*point.geometry().unwrap(), [vec![Point::new(1, 2)]]);
        // A feature without geometry has none.
        let empty = layer.feature(1).unwrap();
        assert!(empty.geometry().unwrap().is_empty());
        assert_eq!(empty.get_value("k"), None);
    }

    /// Outside a codec with tables, data is written as what it is made from.
    #[test]
    fn data_crosses_as_what_it_is_made_from() {
        let round_trip =
            |data: &TileData| -> TileData { mln_wire::from_slice(&mln_wire::to_vec(data).unwrap()).unwrap() };
        let mvt = fixture("0-0-0.mvt");
        let TileData::Mvt(tile) = round_trip(&TileData::Mvt(MvtTile::new(Arc::clone(&mvt)))) else { panic!("not MVT") };
        assert_eq!(tile.bytes(), &mvt);

        let mlt = fixture("0-0-0-fastpfor.mlt");
        let TileData::Mlt(tile) = round_trip(&TileData::Mlt(MltTile::new(Arc::clone(&mlt), true))) else {
            panic!("not MLT")
        };
        assert_eq!((tile.bytes(), tile.fast_pfor_enabled()), (&mlt, true));
        assert!(tile.layer("admin").is_some());

        let TileData::Features(read) = round_trip(&TileData::Features(features())) else { panic!("not features") };
        assert_eq!(read, features());
    }
}
