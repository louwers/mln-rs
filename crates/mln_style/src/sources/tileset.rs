//! Vector, raster and raster-dem sources: tiles from a tileset that the
//! style lists, that the TileJSON at the source's URL gives, or both, the
//! style's members winning. One generic type registers as all three.

use std::marker::PhantomData;
use std::sync::Arc;

use mln_style_types::SourceType;
use mln_util::constants;
use mln_util::tileset::{PartialTileset, Tileset, VectorEncoding};

use crate::conversion::tileset::{convert_tileset, optional_member};
use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::layer_type::SourceKind;
use crate::source_type::{OfflineResources, Source, SourceData, SourceSummary, SourceTypeInfo, StyleSourceType};
use crate::sources::tile_source::TileSource;

/// Which of the tileset source types a [`TilesetSourceType`] is.
pub trait TilesetKind: Send + Sync + 'static {
    const INFO: SourceTypeInfo;
}

pub struct VectorKind;
pub struct RasterKind;
pub struct RasterDemKind;

impl TilesetKind for VectorKind {
    const INFO: SourceTypeInfo =
        SourceTypeInfo { name: "vector", layer_kind: SourceKind::Geometry, from_json: true, type_: SourceType::Vector };
}

impl TilesetKind for RasterKind {
    const INFO: SourceTypeInfo =
        SourceTypeInfo { name: "raster", layer_kind: SourceKind::Raster, from_json: true, type_: SourceType::Raster };
}

impl TilesetKind for RasterDemKind {
    const INFO: SourceTypeInfo = SourceTypeInfo {
        name: "raster-dem",
        layer_kind: SourceKind::RasterDem,
        from_json: true,
        type_: SourceType::RasterDEM,
    };
}

/// A source type whose tiles a tileset lists.
pub struct TilesetSourceType<K>(PhantomData<fn() -> K>);

pub type VectorSourceType = TilesetSourceType<VectorKind>;
pub type RasterSourceType = TilesetSourceType<RasterKind>;
pub type RasterDemSourceType = TilesetSourceType<RasterDemKind>;

/// A tileset source's options.
#[derive(Clone, Debug, PartialEq)]
pub struct TilesetSourceOptions {
    /// The URL of the source's TileJSON.
    pub url: Option<String>,
    /// The style's tileset members, which win over those of the TileJSON at
    /// `url`.
    pub tileset: PartialTileset,
    /// The size of the source's tiles, in pixels.
    pub tile_size: u16,
}

impl TilesetSourceOptions {
    /// The tileset the style lists, for a source without a URL.
    #[must_use]
    pub fn inline_tileset(&self) -> Option<Tileset> {
        self.url.is_none().then(|| self.tileset.clone().into())
    }

    /// The source's tileset once `tilejson`, the TileJSON at its URL, loads:
    /// the style's members over the document's, for a source of `type_`.
    ///
    /// # Errors
    ///
    /// When `tilejson` isn't a valid TileJSON, or neither it nor the style
    /// lists tiles.
    pub fn tileset_with_tilejson(&self, tilejson: &[u8], type_: SourceType) -> Result<Tileset, String> {
        let document: serde_json::Value = serde_json::from_slice(tilejson).map_err(|e| e.to_string())?;
        let document = convert_tileset(&Convertible::from(&document), type_)?;
        let tileset = self.tileset.clone().or(document);
        if tileset.tiles.is_none() {
            return Err("source must have tiles".to_owned());
        }
        Ok(tileset.into())
    }
}

/// The options of a raster, raster-dem or vector source from its style JSON:
/// the TileJSON at its `url`, its own TileJSON members, or both.
fn convert_options(value: &Convertible<'_>, type_: SourceType) -> Result<TilesetSourceOptions, String> {
    let url = optional_member(value, "url", Convertible::to_string)?;
    let tileset = convert_tileset(value, type_)?;
    if url.is_none() && tileset.tiles.is_none() {
        return Err("source must have a url or tiles".to_owned());
    }
    let tile_size = optional_member(value, "tileSize", |size| {
        size.to_number().filter(|size| (0.0..=f32::from(u16::MAX)).contains(size)).map(|size| size as u16)
    })?;
    // A vector source's tiles are always the default size.
    let tile_size =
        if type_ == SourceType::Vector { constants::TILE_SIZE_I } else { tile_size.unwrap_or(constants::TILE_SIZE_I) };
    Ok(TilesetSourceOptions { url, tileset, tile_size })
}

impl<K: TilesetKind> StyleSourceType for TilesetSourceType<K> {
    const INFO: SourceTypeInfo = K::INFO;

    type Options = TilesetSourceOptions;
    type Description = Tileset;

    fn from_json(
        id: &str,
        value: &Convertible<'_>,
        _functions: &Arc<Functions>,
        error: &mut Error,
    ) -> Option<Box<dyn crate::source::Source>> {
        match convert_options(value, K::INFO.type_) {
            Ok(options) => Some(Box::new(TileSource::new(Source::new(id, SourceData::<Self>::new(options))))),
            Err(message) => {
                error.message = message;
                None
            }
        }
    }

    fn attribution(source: &SourceData<Self>) -> Option<String> {
        source.description.as_ref().map(|tileset| tileset.attribution.clone())
    }

    fn summary(source: &SourceData<Self>) -> SourceSummary {
        let options = &source.options;
        let vector_encoding = (K::INFO.type_ == SourceType::Vector).then(|| {
            let loaded = source.description.as_ref().and_then(|tileset| tileset.vector_encoding);
            loaded.or(options.tileset.vector_encoding).unwrap_or(VectorEncoding::Mapbox)
        });
        SourceSummary {
            url: options.url.clone(),
            tile_size: Some(options.tile_size),
            inline_tileset: options.inline_tileset(),
            vector_encoding,
            geojson: None,
        }
    }

    fn offline(source: &SourceData<Self>) -> OfflineResources {
        let options = &source.options;
        match &options.url {
            Some(url) => OfflineResources::TileJson { url: url.clone(), tile_size: options.tile_size },
            None => match options.inline_tileset() {
                Some(tileset) => OfflineResources::Tiles { tileset, tile_size: options.tile_size },
                None => OfflineResources::None,
            },
        }
    }

    fn offline_tileset(source: &SourceData<Self>, tilejson: &[u8]) -> Result<Tileset, String> {
        source.options.tileset_with_tilejson(tilejson, K::INFO.type_)
    }
}

/// The options and loaded tileset of `source`, if it's a vector, raster or
/// raster-dem source.
#[must_use]
pub fn tileset_parts(source: &Source) -> Option<(&TilesetSourceOptions, Option<&Tileset>)> {
    fn parts<K: TilesetKind>(data: &SourceData<TilesetSourceType<K>>) -> (&TilesetSourceOptions, Option<&Tileset>) {
        (&data.options, data.description.as_ref())
    }
    if let Some(data) = SourceData::<VectorSourceType>::recover(source) {
        Some(parts(data))
    } else if let Some(data) = SourceData::<RasterSourceType>::recover(source) {
        Some(parts(data))
    } else {
        SourceData::<RasterDemSourceType>::recover(source).map(parts)
    }
}

/// `source` with `tileset` as its loaded tileset, if it's a vector, raster or
/// raster-dem source.
#[must_use]
pub(crate) fn with_tileset(source: &Source, tileset: Tileset) -> Option<Source> {
    fn with<K: TilesetKind>(source: &Source, tileset: Tileset) -> Option<Source> {
        let data = SourceData::<TilesetSourceType<K>>::recover(source)?;
        Some(source.with_data(SourceData::<TilesetSourceType<K>> { description: Some(tileset), ..data.clone() }))
    }
    if SourceData::<VectorSourceType>::recover(source).is_some() {
        with::<VectorKind>(source, tileset)
    } else if SourceData::<RasterSourceType>::recover(source).is_some() {
        with::<RasterKind>(source, tileset)
    } else {
        with::<RasterDemKind>(source, tileset)
    }
}
