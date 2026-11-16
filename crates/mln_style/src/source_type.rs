//! Source types as values.
//!
//! A source type is a marker type that implements [`StyleSourceType`]. A
//! style holds each of its sources as a [`Source`]: its id, the members every
//! source has ([`SourceCommon`]) and its type's [`SourceData<T>`], shared as
//! an `Arc<dyn ErasedSource>`. A [`StyleRegistry`](crate::StyleRegistry)
//! holds the types as `&'static dyn ErasedStyleSourceType`, by name. An
//! erased value gets back to its type only through [`SourceData::recover`]
//! and [`SourceData::recover_arc`].

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use mln_conversion::{Convertible, Error};
use mln_style_types::SourceType;
use mln_util::constants;
use mln_util::logging::{Event, Log};
use mln_util::tileset::{Tileset, VectorEncoding};

use crate::expression::Functions;
use crate::layer_type::SourceKind;
use crate::sources::geojson::GeoJSONOptions;

/// What the engine needs to know about a source type besides its data. The
/// name identifies the type: two registries that agree on a name agree on
/// the type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceTypeInfo {
    /// The source type as style JSON names it, or as the API does for a type
    /// that style JSON can't create.
    pub name: &'static str,
    /// The kind of layer that the type's sources feed.
    pub layer_kind: SourceKind,
    /// Whether style JSON can create a source of the type.
    pub from_json: bool,
    /// The type as [`SourceType`] names it, which the tile cover and the
    /// offline tile count still read.
    pub type_: SourceType,
}

/// What a source reports about itself, whichever its type. A member that
/// the type doesn't have is `None`.
#[derive(Clone, Debug, Default)]
pub struct SourceSummary {
    /// The URL that the source loads its description from.
    pub url: Option<String>,
    /// The size of the source's tiles, in pixels.
    pub tile_size: Option<u16>,
    /// The tileset that the style lists, for a tileset source without a URL.
    pub inline_tileset: Option<Tileset>,
    /// How a vector source's tiles are encoded: as its loaded tileset says,
    /// else as the style does.
    pub vector_encoding: Option<VectorEncoding>,
    /// A GeoJSON source's options.
    pub geojson: Option<Arc<GeoJSONOptions>>,
}

/// What an offline download of a source fetches.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum OfflineResources {
    /// Nothing: the host gives the source's data.
    #[default]
    None,
    /// The tiles of a tileset that's known without a fetch, of tiles
    /// `tile_size` pixels wide.
    Tiles { tileset: Tileset, tile_size: u16 },
    /// The TileJSON at `url`, then the tiles, `tile_size` pixels wide, of
    /// the tileset that [`ErasedSource::offline_tileset`] makes of it.
    TileJson { url: String, tile_size: u16 },
    /// The source's data at `url`: GeoJSON.
    Source { url: String },
    /// The source's image at `url`, which is nothing to fetch if empty.
    Image { url: String },
}

/// A source type: what its sources hold, how style JSON describes them and
/// what they report.
pub trait StyleSourceType: Sized + Send + Sync + 'static {
    const INFO: SourceTypeInfo;

    /// What the style or the API gives: a URL, an inline tileset, GeoJSON
    /// options, coordinates.
    type Options: Clone + Send + Sync + 'static;

    /// What loading finds: a tileset, GeoJSON data, an image, a tile loader.
    type Description: Clone + Send + Sync + 'static;

    /// A source named `id` from its style JSON `value`, whose expressions
    /// parse against `functions`, as the handle that loads it. Only a type
    /// whose [`INFO`](Self::INFO) has `from_json` is asked.
    fn from_json(
        _id: &str,
        _value: &Convertible<'_>,
        _functions: &Arc<Functions>,
        error: &mut Error,
    ) -> Option<Box<dyn crate::source::Source>> {
        error.message = "invalid source type".to_owned();
        None
    }

    fn attribution(_source: &SourceData<Self>) -> Option<String> {
        None
    }

    fn summary(_source: &SourceData<Self>) -> SourceSummary {
        SourceSummary::default()
    }

    /// Whether the source's tiles lay out in the call that asks for them.
    fn is_update_synchronous(_source: &SourceData<Self>) -> bool {
        false
    }

    /// What an offline download fetches for the source.
    fn offline(_source: &SourceData<Self>) -> OfflineResources {
        OfflineResources::None
    }

    /// The tileset that an offline download takes the source's tiles from,
    /// once the TileJSON that [`OfflineResources::TileJson`] named arrived.
    ///
    /// # Errors
    ///
    /// When `tilejson` doesn't give the source a tileset.
    fn offline_tileset(_source: &SourceData<Self>, _tilejson: &[u8]) -> Result<Tileset, String> {
        Err(format!("a {} source has no TileJSON", Self::INFO.name))
    }
}

fn warn_if_overscale_factor_caps_prefetch_delta(overscale: Option<u8>, prefetch: Option<u8>) {
    let prefetch_delta = std::cmp::max(constants::DEFAULT_PREFETCH_ZOOM_DELTA, prefetch.unwrap_or(0));
    if let Some(overscale) = overscale
        && overscale < prefetch_delta
    {
        Log::warning(
            Event::Style,
            format!("Parent tile overscale factor will cap prefetch delta to {}", i32::from(overscale)),
        );
    }
}

/// The members every source has, which tune how its tiles load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceCommon {
    prefetch_zoom_delta: Option<u8>,
    max_overscale_factor: Option<u8>,
    minimum_tile_update_interval: Duration,
    volatile: bool,
}

impl SourceCommon {
    pub(crate) fn set_prefetch_zoom_delta(&mut self, delta: Option<u8>) {
        self.prefetch_zoom_delta = delta;
        warn_if_overscale_factor_caps_prefetch_delta(self.max_overscale_factor, self.prefetch_zoom_delta);
    }

    #[must_use]
    pub fn get_prefetch_zoom_delta(&self) -> Option<u8> {
        self.prefetch_zoom_delta
    }

    pub(crate) fn set_minimum_tile_update_interval(&mut self, interval: Duration) {
        self.minimum_tile_update_interval = interval;
    }

    #[must_use]
    pub fn get_minimum_tile_update_interval(&self) -> Duration {
        self.minimum_tile_update_interval
    }

    pub(crate) fn set_max_overscale_factor_for_parent_tiles(&mut self, overscale_factor: Option<u8>) {
        self.max_overscale_factor = overscale_factor;
        warn_if_overscale_factor_caps_prefetch_delta(self.max_overscale_factor, self.prefetch_zoom_delta);
    }

    #[must_use]
    pub fn get_max_overscale_factor_for_parent_tiles(&self) -> Option<u8> {
        self.max_overscale_factor
    }

    #[must_use]
    pub fn is_volatile(&self) -> bool {
        self.volatile
    }

    pub(crate) fn set_volatile(&mut self, set: bool) {
        self.volatile = set;
    }
}

/// The data of a source of the type `T`: its options and what loading them
/// found.
pub struct SourceData<T: StyleSourceType> {
    pub options: T::Options,
    /// `None` until the source's description loads.
    pub description: Option<T::Description>,
    /// Grows with each change to the options or the data that starts a new
    /// load, and tags that load.
    pub load: u64,
}

impl<T: StyleSourceType> Clone for SourceData<T> {
    fn clone(&self) -> Self {
        SourceData { options: self.options.clone(), description: self.description.clone(), load: self.load }
    }
}

impl<T: StyleSourceType> SourceData<T> {
    /// A source of `options` whose description hasn't loaded.
    #[must_use]
    pub fn new(options: T::Options) -> Self {
        SourceData { options, description: None, load: 0 }
    }

    /// `source`'s data as a `T` source's, or `None` for another type's.
    #[must_use]
    pub fn recover(source: &Source) -> Option<&Self> {
        let any: &dyn Any = &*source.data;
        any.downcast_ref()
    }

    /// `source`'s data as a `T` source's, sharing its allocation, or `None`
    /// for another type's.
    #[must_use]
    pub fn recover_arc(source: &Source) -> Option<Arc<Self>> {
        let any: Arc<dyn Any + Send + Sync> = Arc::<dyn ErasedSource>::clone(&source.data);
        any.downcast().ok()
    }
}

/// A source's data, whichever its type.
pub trait ErasedSource: Any + Send + Sync {
    fn source_type(&self) -> &'static dyn ErasedStyleSourceType;

    fn attribution(&self) -> Option<String>;

    fn summary(&self) -> SourceSummary;

    fn is_update_synchronous(&self) -> bool;

    fn offline(&self) -> OfflineResources;

    /// [`StyleSourceType::offline_tileset`].
    ///
    /// # Errors
    ///
    /// When `tilejson` doesn't give the source a tileset.
    fn offline_tileset(&self, tilejson: &[u8]) -> Result<Tileset, String>;
}

impl<T: StyleSourceType> ErasedSource for SourceData<T> {
    fn source_type(&self) -> &'static dyn ErasedStyleSourceType {
        style_source_entry::<T>()
    }

    fn attribution(&self) -> Option<String> {
        T::attribution(self)
    }

    fn summary(&self) -> SourceSummary {
        T::summary(self)
    }

    fn is_update_synchronous(&self) -> bool {
        T::is_update_synchronous(self)
    }

    fn offline(&self) -> OfflineResources {
        T::offline(self)
    }

    fn offline_tileset(&self, tilejson: &[u8]) -> Result<Tileset, String> {
        T::offline_tileset(self, tilejson)
    }
}

/// A source of a style version: its id, the members every source has, and
/// its type's data. A clone shares the data; two sources are the same when
/// [`Source::ptr_eq`] says so.
#[derive(Clone)]
pub struct Source {
    id: Arc<str>,
    common: SourceCommon,
    data: Arc<dyn ErasedSource>,
}

impl Source {
    /// The source `id` of `data`, with the default common members.
    #[must_use]
    pub fn new<T: StyleSourceType>(id: &str, data: SourceData<T>) -> Self {
        Source { id: Arc::from(id), common: SourceCommon::default(), data: Arc::new(data) }
    }

    /// This source with `data` in place of its data.
    #[must_use]
    pub fn with_data<T: StyleSourceType>(&self, data: SourceData<T>) -> Self {
        Source { id: Arc::clone(&self.id), common: self.common, data: Arc::new(data) }
    }

    /// This source with its data changed by `f`, or `None` if it isn't a
    /// `T` source.
    #[must_use]
    pub fn map_data<T: StyleSourceType>(&self, f: impl FnOnce(&mut SourceData<T>)) -> Option<Self> {
        let mut data = SourceData::<T>::recover(self)?.clone();
        f(&mut data);
        Some(self.with_data(data))
    }

    /// This source with `common` in place of its common members.
    #[must_use]
    pub fn with_common(&self, common: SourceCommon) -> Self {
        Source { id: Arc::clone(&self.id), common, data: Arc::clone(&self.data) }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn info(&self) -> SourceTypeInfo {
        self.data.source_type().info()
    }

    #[must_use]
    pub fn source_type(&self) -> &'static dyn ErasedStyleSourceType {
        self.data.source_type()
    }

    #[must_use]
    pub fn get_type(&self) -> SourceType {
        self.info().type_
    }

    #[must_use]
    pub fn common(&self) -> &SourceCommon {
        &self.common
    }

    /// The source's type's data.
    #[must_use]
    pub fn data(&self) -> &dyn ErasedSource {
        &*self.data
    }

    #[must_use]
    pub fn get_attribution(&self) -> Option<String> {
        self.data.attribution()
    }

    #[must_use]
    pub fn summary(&self) -> SourceSummary {
        self.data.summary()
    }

    #[must_use]
    pub fn is_update_synchronous(&self) -> bool {
        self.data.is_update_synchronous()
    }

    /// Whether `a` and `b` are the same source: the same id and common
    /// members, and data that's the same allocation.
    #[must_use]
    pub fn ptr_eq(a: &Source, b: &Source) -> bool {
        Arc::ptr_eq(&a.data, &b.data) && a.common == b.common && a.id == b.id
    }
}

// Debug is not a supertrait, for the reason at
// `impl Debug for dyn ErasedLayerStyle` (layer_type.rs).
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("id", &self.id)
            .field("type", &self.info().name)
            .field("common", &self.common)
            .finish_non_exhaustive()
    }
}

/// A source type, whichever it is.
pub trait ErasedStyleSourceType: Send + Sync + 'static {
    fn info(&self) -> SourceTypeInfo;

    /// [`StyleSourceType::from_json`].
    fn create_from_json(
        &self,
        id: &str,
        value: &Convertible<'_>,
        functions: &Arc<Functions>,
        error: &mut Error,
    ) -> Option<Box<dyn crate::source::Source>>;
}

/// The [`ErasedStyleSourceType`] of `T`, which [`style_source_entry`]
/// returns.
pub struct SourceTypeEntry<T>(PhantomData<fn() -> T>);

impl<T: 'static> SourceTypeEntry<T> {
    pub const INSTANCE: &'static Self = &SourceTypeEntry(PhantomData);
}

impl<T: StyleSourceType> ErasedStyleSourceType for SourceTypeEntry<T> {
    fn info(&self) -> SourceTypeInfo {
        T::INFO
    }

    fn create_from_json(
        &self,
        id: &str,
        value: &Convertible<'_>,
        functions: &Arc<Functions>,
        error: &mut Error,
    ) -> Option<Box<dyn crate::source::Source>> {
        T::from_json(id, value, functions, error)
    }
}

/// The source type `T` as a value.
#[must_use]
pub const fn style_source_entry<T: StyleSourceType>() -> &'static dyn ErasedStyleSourceType {
    SourceTypeEntry::<T>::INSTANCE
}
