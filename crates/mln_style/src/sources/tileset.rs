//! Vector, raster and raster-dem sources: tiles from a tileset that the
//! style lists, that the TileJSON at the source's URL gives, or both, the
//! style's members winning. One generic type registers as all three.

use std::marker::PhantomData;
use std::sync::Arc;

use crate::constants;
use crate::conversion::tileset::{convert_tileset, optional_member};
use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::layer::SourceKind;
use crate::source::{
    Data, Described, Fetch, Load, OfflineResources, Respond, Source, SourceData, SourceSummary, SourceTypeInfo,
    StyleSourceType,
};
use crate::tileset::{PartialTileset, Tileset, VectorEncoding};

/// Which of the tileset source types a [`TilesetSourceType`] is.
pub trait TilesetKind: Send + Sync + 'static {
    const INFO: SourceTypeInfo;
}

pub struct VectorKind;
pub struct RasterKind;
pub struct RasterDemKind;

impl TilesetKind for VectorKind {
    const INFO: SourceTypeInfo = SourceTypeInfo {
        name: "vector",
        layer_kind: SourceKind::Geometry,
        from_json: true,
        round_zoom: false,
        overscale: true,
        prefetch: true,
    };
}

impl TilesetKind for RasterKind {
    const INFO: SourceTypeInfo = SourceTypeInfo {
        name: "raster",
        layer_kind: SourceKind::Raster,
        from_json: true,
        round_zoom: true,
        overscale: false,
        prefetch: true,
    };
}

impl TilesetKind for RasterDemKind {
    const INFO: SourceTypeInfo = SourceTypeInfo {
        name: "raster-dem",
        layer_kind: SourceKind::RasterDem,
        from_json: true,
        round_zoom: true,
        // As a raster source's: a DEM tile is the same at every zoom past
        // the maxzoom, so the map draws it larger rather than load it again.
        overscale: false,
        prefetch: true,
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
    /// the style's members over the document's, for a source whose tiles
    /// are of the kind `tiles`.
    ///
    /// # Errors
    ///
    /// When `tilejson` isn't a valid TileJSON, or neither it nor the style
    /// lists tiles.
    pub fn tileset_with_tilejson(&self, tilejson: &[u8], tiles: SourceKind) -> Result<Tileset, String> {
        let document: serde_json::Value = serde_json::from_slice(tilejson).map_err(|e| e.to_string())?;
        let document = convert_tileset(&Convertible::from(&document), tiles)?;
        let tileset = self.tileset.clone().or(document);
        if tileset.tiles.is_none() {
            return Err("source must have tiles".to_owned());
        }
        Ok(tileset.into())
    }
}

/// The options of a raster, raster-dem or vector source, whose tiles are of
/// the kind `tiles`, from its style JSON: the TileJSON at its `url`, its own
/// TileJSON members, or both.
fn convert_options(value: &Convertible<'_>, tiles: SourceKind) -> Result<TilesetSourceOptions, String> {
    let url = optional_member(value, "url", Convertible::to_string)?;
    let tileset = convert_tileset(value, tiles)?;
    if url.is_none() && tileset.tiles.is_none() {
        return Err("source must have a url or tiles".to_owned());
    }
    let tile_size = optional_member(value, "tileSize", |size| {
        size.to_number().filter(|size| (0.0..=f32::from(u16::MAX)).contains(size)).map(|size| size as u16)
    })?;
    // A vector source's tiles are always the default size.
    let tile_size = if tiles == SourceKind::Geometry {
        constants::TILE_SIZE_I
    } else {
        tile_size.unwrap_or(constants::TILE_SIZE_I)
    };
    Ok(TilesetSourceOptions { url, tileset, tile_size })
}

impl<K: TilesetKind> StyleSourceType for TilesetSourceType<K> {
    const INFO: SourceTypeInfo = K::INFO;

    type Options = TilesetSourceOptions;
    type Description = Tileset;

    fn from_json(value: &Convertible<'_>, _functions: &Arc<Functions>, error: &mut Error) -> Option<SourceData<Self>> {
        match convert_options(value, K::INFO.layer_kind) {
            Ok(options) => Some(SourceData::new(options)),
            Err(message) => {
                error.message = message;
                None
            }
        }
    }

    /// The TileJSON at the source's URL, or else the tileset the style lists.
    fn load(source: &SourceData<Self>) -> Load<Tileset> {
        match &source.options.url {
            Some(url) => Load::Fetch(Fetch::Source(url.clone())),
            None => Load::Ready(source.options.tileset.clone().into()),
        }
    }

    fn describe(options: &TilesetSourceOptions, data: Option<Data>, respond: Respond<Described<Tileset>>) {
        let tileset = match data {
            None => Err("unexpectedly empty TileJSON".to_owned()),
            Some(data) => options.tileset_with_tilejson(&data, K::INFO.layer_kind),
        };
        respond(tileset.map(Some).map_err(crate::error::Error::native));
    }

    fn metadata_changed(old: Option<&Tileset>, new: Option<&Tileset>) -> bool {
        old != new
    }

    fn attribution(source: &SourceData<Self>) -> Option<String> {
        source.description.as_ref().map(|tileset| tileset.attribution.clone())
    }

    fn summary(source: &SourceData<Self>) -> SourceSummary {
        let options = &source.options;
        let vector_encoding = (K::INFO.layer_kind == SourceKind::Geometry).then(|| {
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
        source.options.tileset_with_tilejson(tilejson, K::INFO.layer_kind)
    }
}

fn tileset_source<K: TilesetKind>(id: &str, url: Option<String>, tileset: PartialTileset, tile_size: u16) -> Source {
    Source::new(id, SourceData::<TilesetSourceType<K>>::new(TilesetSourceOptions { url, tileset, tile_size }))
}

/// A vector source: the TileJSON at `url`, the members of `tileset`, or
/// both, `tileset`'s winning.
#[must_use]
pub fn vector_source(id: &str, url: Option<String>, tileset: PartialTileset) -> Source {
    tileset_source::<VectorKind>(id, url, tileset, constants::TILE_SIZE_I)
}

/// A raster source of tiles `tile_size` pixels wide, as [`vector_source`].
#[must_use]
pub fn raster_source(id: &str, url: Option<String>, tileset: PartialTileset, tile_size: u16) -> Source {
    tileset_source::<RasterKind>(id, url, tileset, tile_size)
}

/// A raster-dem source of tiles `tile_size` pixels wide, as
/// [`vector_source`].
#[must_use]
pub fn raster_dem_source(id: &str, url: Option<String>, tileset: PartialTileset, tile_size: u16) -> Source {
    tileset_source::<RasterDemKind>(id, url, tileset, tile_size)
}
