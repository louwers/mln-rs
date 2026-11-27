//! GeoJSON sources: features from GeoJSON data that the style or the API
//! gives, or that the source's URL points to, cut into tiles or clustered.

use std::sync::{Arc, OnceLock};

use geojson::GeoJson;

use crate::collections::VecMap;
use crate::conversion::{Convertible, Error as ConversionError};
use crate::expression::{Expr, Functions};
use crate::layer::SourceKind;
use crate::range::Range;
use crate::source::{Source, SourceData, SourceTypeInfo, StyleSourceType};
use crate::{Edit, constants};

/// A `clusterProperties` member: its map expression, then its reduce
/// expression.
pub type ClusterExpression = (Arc<Expr>, Arc<Expr>);
pub type ClusterProperties = VecMap<String, ClusterExpression>;

#[derive(Clone)]
pub struct GeoJSONOptions {
    // GeoJSON-VT options
    pub minzoom: u8,
    pub maxzoom: u8,
    pub tile_size: u16,
    pub buffer: u16,
    pub tolerance: f64,
    pub line_metrics: bool,

    // Clustering options
    pub cluster: bool,
    pub cluster_radius: u16,
    pub cluster_max_zoom: u8,
    pub cluster_min_points: usize,
    pub cluster_properties: ClusterProperties,

    // Update options
    pub synchronous_update: bool,

    /// The style's attribution for the source, reported by
    /// `Source::get_attribution`.
    pub attribution: Option<String>,
}

impl Default for GeoJSONOptions {
    fn default() -> Self {
        GeoJSONOptions {
            minzoom: 0,
            maxzoom: 18,
            tile_size: constants::TILE_SIZE_I,
            buffer: 128,
            tolerance: 0.375,
            line_metrics: false,
            cluster: false,
            cluster_radius: 50,
            cluster_max_zoom: 17,
            cluster_min_points: 2,
            cluster_properties: ClusterProperties::new(),
            synchronous_update: false,
            attribution: None,
        }
    }
}

impl std::fmt::Debug for GeoJSONOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoJSONOptions")
            .field("minzoom", &self.minzoom)
            .field("maxzoom", &self.maxzoom)
            .field("tile_size", &self.tile_size)
            .field("buffer", &self.buffer)
            .field("tolerance", &self.tolerance)
            .field("line_metrics", &self.line_metrics)
            .field("cluster", &self.cluster)
            .field("cluster_radius", &self.cluster_radius)
            .field("cluster_max_zoom", &self.cluster_max_zoom)
            .field("cluster_min_points", &self.cluster_min_points)
            .field("cluster_properties", &self.cluster_properties.keys().collect::<Vec<_>>())
            .field("synchronous_update", &self.synchronous_update)
            .field("attribution", &self.attribution)
            .finish()
    }
}

impl GeoJSONOptions {
    /// The default options, as one shared instance.
    pub fn default_options() -> Arc<GeoJSONOptions> {
        static OPTIONS: OnceLock<Arc<GeoJSONOptions>> = OnceLock::new();
        Arc::clone(OPTIONS.get_or_init(|| Arc::new(GeoJSONOptions::default())))
    }
}

/// The GeoJSON source type.
pub struct GeoJsonSourceType;

/// GeoJSON that the style gives as a source's `data`, or why it isn't
/// GeoJSON, which fails the source as it loads rather than the style.
pub type InlineGeoJson = Arc<Result<GeoJson, String>>;

/// A GeoJSON source's options.
#[derive(Clone, Debug)]
pub struct GeoJsonSourceOptions {
    pub options: Arc<GeoJSONOptions>,
    /// The URL of the source's GeoJSON, if it loads it from one.
    pub url: Option<String>,
    /// The GeoJSON the style gives, if it gives it rather than a URL.
    pub inline: Option<InlineGeoJson>,
    /// `options.synchronous_update` until the source's override replaces it.
    pub synchronous_update: bool,
}

impl GeoJsonSourceOptions {
    #[must_use]
    pub fn new(options: Arc<GeoJSONOptions>) -> Self {
        let synchronous_update = options.synchronous_update;
        GeoJsonSourceOptions { options, url: None, inline: None, synchronous_update }
    }

    #[must_use]
    pub fn get_zoom_range(&self) -> Range<u8> {
        Range::new(self.options.minzoom, self.options.maxzoom)
    }
}

impl StyleSourceType for GeoJsonSourceType {
    const INFO: SourceTypeInfo = SourceTypeInfo { name: "geojson", layer_kind: SourceKind::Geometry, from_json: true };

    type Options = GeoJsonSourceOptions;

    fn from_json(
        value: &Convertible<'_>,
        functions: &Arc<Functions>,
        error: &mut ConversionError,
    ) -> Option<SourceData<Self>> {
        crate::conversion::source::convert_geojson_source(value, functions, error)
    }
}

impl GeoJsonSourceType {
    /// The edit that makes the GeoJSON source `id` load its data from
    /// `url`. Its data stays until the URL's arrives.
    pub fn set_url(id: impl Into<String>, url: impl Into<String>) -> Edit {
        let url = url.into();
        Edit::update_source::<Self>(id, "a GeoJSON source", |data| {
            data.options.url = Some(url);
            data.options.inline = None;
            data.load += 1;
            true
        })
    }

    /// The edit that sets whether the tiles of the GeoJSON source `id` lay
    /// out in the call that asks for them, in place of its
    /// `synchronous_update` option.
    pub fn set_synchronous(id: impl Into<String>, synchronous: bool) -> Edit {
        Edit::update_source::<Self>(id, "a GeoJSON source", move |data| {
            let changed = data.options.synchronous_update != synchronous;
            data.options.synchronous_update = synchronous;
            changed
        })
    }
}

/// A GeoJSON source with `options` and no data until it gets some or a URL.
#[must_use]
pub fn geojson_source(id: &str, options: Arc<GeoJSONOptions>) -> Source {
    Source::new(id, SourceData::<GeoJsonSourceType>::new(GeoJsonSourceOptions::new(options)))
}
