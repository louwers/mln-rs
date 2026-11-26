//! Source types as values.
//!
//! A source type is a marker type that implements [`StyleSourceType`]. A
//! style holds each of its sources as a [`Source`]: its id, the members every
//! source has ([`SourceCommon`]) and its type's [`SourceData<T>`], shared as
//! an `Arc<dyn ErasedSource>`. A [`StyleRegistry`](crate::StyleRegistry)
//! holds the types as `&'static dyn ErasedStyleSourceType`, by name. An
//! erased value gets back to its type only through [`SourceData::recover`].
//!
//! A source's description is what loading its options finds, such as the
//! tileset of a TileJSON. The type says how it loads ([`StyleSourceType::load`]
//! and [`StyleSourceType::describe`]); the map's loader runs the loads and
//! applies what they find as an [`Edit::SetSourceDescription`](crate::Edit).

use std::any::Any;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::bitmap::PremultipliedImage;
use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::layer::SourceKind;
use crate::sources::geojson::GeoJSONOptions;
use crate::tileset::{Tileset, VectorEncoding};
use crate::{constants, log_target};

mod convert;
mod list;

pub use convert::convert_source;
pub use list::SourceList;

/// Where a job hands its result: called at most once, from any thread,
/// while the job runs or later. Dropping it uncalled drops the result.
pub type Respond<R> = Box<dyn FnOnce(R) + Send>;

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
    /// Whether the tiles that cover a view are of the zoom level nearest the
    /// view's, as for raster tiles, rather than the level below it. A
    /// render in tile mode then draws every tile of the cover, not only
    /// the first.
    pub round_zoom: bool,
    /// Whether a view past the highest zoom level that has tiles asks for
    /// overscaled tiles of its own zoom, rather than drawing that level's
    /// tiles larger.
    pub overscale: bool,
    /// Whether, in continuous mode, the map first loads tiles a few zoom
    /// levels below the view's, and keeps the globe's lowest level loaded,
    /// so that something shows while the view's tiles load.
    pub prefetch: bool,
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

/// How a source's description loads, as its type says from its data.
pub enum Load<D> {
    /// Nothing loads: the source is loaded with the description it has.
    Done,
    /// The description, known without a fetch, such as a tileset that the
    /// style lists.
    Ready(D),
    /// The resource that [`StyleSourceType::describe`] or, for an image,
    /// [`StyleSourceType::describe_image`] turns into the description.
    Fetch(Fetch),
}

/// A resource that a source loads its description from, by its URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// A document that describes the source, such as TileJSON or GeoJSON.
    Source(String),
    /// An image, which the map's loader decodes.
    Image(String),
}

/// The bytes that a source loads from: a fetched resource's, or those of
/// a tile that the host gives.
pub type Data = Arc<Vec<u8>>;

/// What describing a source's fetched data found: its description, `None`
/// for data that describes nothing, or why the source failed to load.
pub type Described<D> = Result<Option<D>, crate::error::Error>;

/// A description of a source of some type, which
/// [`Source::with_description`] gives back to its type.
pub type ErasedDescription = Box<dyn Any + Send>;

/// A source type: what its sources hold, how style JSON describes them, how
/// their descriptions load and what they report.
pub trait StyleSourceType: Sized + Send + Sync + 'static {
    const INFO: SourceTypeInfo;

    /// Whether [`describe`](Self::describe) runs on the thread pool rather
    /// than on the map's thread, for work such as parsing GeoJSON.
    const DESCRIBES_IN_BACKGROUND: bool = false;

    /// What the style or the API gives: a URL, an inline tileset, GeoJSON
    /// options, coordinates.
    type Options: Clone + Send + Sync + 'static;

    /// What loading finds: a tileset, GeoJSON data, an image.
    type Description: Clone + Send + Sync + 'static;

    /// The data of a source from its style JSON `value`, whose expressions
    /// parse against `functions`. Only a type whose [`INFO`](Self::INFO)
    /// has `from_json` is asked.
    fn from_json(_value: &Convertible<'_>, _functions: &Arc<Functions>, error: &mut Error) -> Option<SourceData<Self>> {
        error.message = "invalid source type".to_owned();
        None
    }

    /// How `source`'s description loads for its current options. The map
    /// asks again whenever [`SourceData::load`] grows.
    fn load(_source: &SourceData<Self>) -> Load<Self::Description> {
        Load::Done
    }

    /// Turns `data`, the fetched resource of a [`Fetch::Source`], into the
    /// description of a source of `options`, and answers through `respond`,
    /// at once or later. `None` is a response without content.
    fn describe(_options: &Self::Options, _data: Option<Data>, respond: Respond<Described<Self::Description>>) {
        respond(Err(crate::error::Error::native(format!("a {} source fetches nothing", Self::INFO.name))));
    }

    /// Turns `image`, the decoded image of a [`Fetch::Image`], into the
    /// description of a source of `options`.
    fn describe_image(_options: &Self::Options, _image: PremultipliedImage) -> Described<Self::Description> {
        Err(crate::error::Error::native(format!("a {} source fetches no image", Self::INFO.name)))
    }

    /// Whether `new`, a loaded description that replaces `old`, changes what
    /// the source says about itself, such as a tileset's tiles and
    /// attribution, which the map reports as a change of the source.
    #[must_use]
    fn metadata_changed(_old: Option<&Self::Description>, _new: Option<&Self::Description>) -> bool {
        false
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
        log::warn!(
            target: log_target::STYLE,
            "Parent tile overscale factor will cap prefetch delta to {}",
            i32::from(overscale)
        );
    }
}

/// The members every source has, which tune how its tiles load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceCommon {
    /// How many zoom levels below the ideal ones to load tiles of first;
    /// `None` uses the map's.
    pub prefetch_zoom_delta: Option<u8>,
    /// How many zoom levels a parent tile may stand in for its children
    /// while they load; `None` doesn't limit it.
    pub max_overscale_factor: Option<u8>,
    /// How long a tile waits before it reloads after its data expires.
    pub minimum_tile_update_interval: Duration,
    /// Whether the source's tiles stay out of the offline cache.
    pub volatile: bool,
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

    /// A source of `options` with `description`.
    #[must_use]
    pub fn with_description(options: T::Options, description: T::Description) -> Self {
        SourceData { options, description: Some(description), load: 0 }
    }

    /// `source`'s data as a `T` source's, or `None` for another type's.
    #[must_use]
    pub fn recover(source: &Source) -> Option<&Self> {
        let any: &dyn Any = &*source.data;
        any.downcast_ref()
    }
}

/// A source's data, whichever its type.
pub trait ErasedSource: Any + Send + Sync {
    fn source_type(&self) -> &'static dyn ErasedStyleSourceType;

    /// [`SourceData::load`].
    fn load(&self) -> u64;

    /// [`StyleSourceType::load`], with the description erased.
    fn plan_load(&self) -> Load<ErasedDescription>;

    /// [`StyleSourceType::DESCRIBES_IN_BACKGROUND`].
    fn describes_in_background(&self) -> bool;

    /// [`StyleSourceType::describe`] of `data` for the source's options, as
    /// a job that owns what it needs and may run on another thread.
    fn describe_job(&self, data: Option<Data>) -> DescribeJob;

    /// [`StyleSourceType::describe_image`] for the source's options, as a
    /// job that owns what it needs and may run on another thread.
    fn describe_image_job(&self) -> DescribeImageJob;

    /// This data with `description` in place of its description, or `None`
    /// if `description` isn't one of the type's.
    fn with_description(&self, description: Option<ErasedDescription>) -> Option<Arc<dyn ErasedSource>>;

    /// [`StyleSourceType::metadata_changed`] from `previous`'s description
    /// to this one's; false when `previous` is another type's.
    fn metadata_changed_from(&self, previous: &dyn ErasedSource) -> bool;

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

/// A job that describes a source's fetched data, answering through the
/// [`Respond`] it takes.
pub type DescribeJob = Box<dyn FnOnce(Respond<Described<ErasedDescription>>) + Send>;

/// A job that describes a source's decoded image.
pub type DescribeImageJob = Box<dyn FnOnce(PremultipliedImage) -> Described<ErasedDescription> + Send>;

fn erase<D: Send + 'static>(description: D) -> ErasedDescription {
    Box::new(description)
}

impl<T: StyleSourceType> ErasedSource for SourceData<T> {
    fn source_type(&self) -> &'static dyn ErasedStyleSourceType {
        style_source_entry::<T>()
    }

    fn load(&self) -> u64 {
        self.load
    }

    fn plan_load(&self) -> Load<ErasedDescription> {
        match T::load(self) {
            Load::Done => Load::Done,
            Load::Ready(description) => Load::Ready(erase(description)),
            Load::Fetch(fetch) => Load::Fetch(fetch),
        }
    }

    fn describes_in_background(&self) -> bool {
        T::DESCRIBES_IN_BACKGROUND
    }

    fn describe_job(&self, data: Option<Data>) -> DescribeJob {
        let options = self.options.clone();
        Box::new(move |respond| {
            T::describe(&options, data, Box::new(move |described| respond(described.map(|d| d.map(erase)))));
        })
    }

    fn describe_image_job(&self) -> DescribeImageJob {
        let options = self.options.clone();
        Box::new(move |image| T::describe_image(&options, image).map(|d| d.map(erase)))
    }

    fn with_description(&self, description: Option<ErasedDescription>) -> Option<Arc<dyn ErasedSource>> {
        let description = match description {
            Some(description) => Some(*description.downcast::<T::Description>().ok()?),
            None => None,
        };
        Some(Arc::new(SourceData::<T> { options: self.options.clone(), description, load: self.load }))
    }

    fn metadata_changed_from(&self, previous: &dyn ErasedSource) -> bool {
        let any: &dyn Any = previous;
        any.downcast_ref::<Self>()
            .is_some_and(|previous| T::metadata_changed(previous.description.as_ref(), self.description.as_ref()))
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

/// Which source a [`Source`] is, of all the sources made in the process:
/// the versions of one source share it, and two sources that only share an
/// id don't.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceInstance(NonZeroU64);

impl SourceInstance {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let value = NEXT.fetch_add(1, Ordering::Relaxed);
        SourceInstance(NonZeroU64::new(value).unwrap_or(NonZeroU64::MIN))
    }
}

/// A source of a style version: its id, which instance it is, the members
/// every source has, and its type's data. A clone shares the data; two
/// sources are the same when [`Source::ptr_eq`] says so.
#[derive(Clone)]
pub struct Source {
    id: Arc<str>,
    instance: SourceInstance,
    common: SourceCommon,
    data: Arc<dyn ErasedSource>,
}

impl Source {
    /// A new source `id` of `data`, with the default common members.
    #[must_use]
    pub fn new<T: StyleSourceType>(id: &str, data: SourceData<T>) -> Self {
        Source {
            id: Arc::from(id),
            instance: SourceInstance::next(),
            common: SourceCommon::default(),
            data: Arc::new(data),
        }
    }

    /// This source with `data` in place of its data.
    #[must_use]
    pub fn with_data<T: StyleSourceType>(&self, data: SourceData<T>) -> Self {
        self.with_erased_data(Arc::new(data))
    }

    fn with_erased_data(&self, data: Arc<dyn ErasedSource>) -> Self {
        Source { id: Arc::clone(&self.id), instance: self.instance, common: self.common, data }
    }

    /// This source with `description` as its description, or `None` if
    /// `description` isn't one of its type's.
    #[must_use]
    pub fn with_description(&self, description: Option<ErasedDescription>) -> Option<Self> {
        Some(self.with_erased_data(self.data.with_description(description)?))
    }

    /// This source with `common` in place of its common members. Logs a
    /// warning when its overscale factor caps its prefetch delta.
    #[must_use]
    pub fn with_common(&self, common: SourceCommon) -> Self {
        warn_if_overscale_factor_caps_prefetch_delta(common.max_overscale_factor, common.prefetch_zoom_delta);
        Source { id: Arc::clone(&self.id), instance: self.instance, common, data: Arc::clone(&self.data) }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn instance(&self) -> SourceInstance {
        self.instance
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
    pub fn common(&self) -> &SourceCommon {
        &self.common
    }

    /// The source's type's data.
    #[must_use]
    pub fn data(&self) -> &dyn ErasedSource {
        &*self.data
    }

    /// Whether `a` and `b` are the same source: the same id, instance and
    /// common members, and data that's the same allocation.
    #[must_use]
    pub fn ptr_eq(a: &Source, b: &Source) -> bool {
        Arc::ptr_eq(&a.data, &b.data) && a.common == b.common && a.instance == b.instance && a.id == b.id
    }

    /// Whether `self`, a later version of `previous`, says something new
    /// about itself through its description (see
    /// [`StyleSourceType::metadata_changed`]).
    #[must_use]
    pub fn metadata_changed_from(&self, previous: &Source) -> bool {
        self.data.metadata_changed_from(&*previous.data)
    }
}

// Debug is not a supertrait, for the reason at
// `impl Debug for dyn ErasedLayerStyle` (layer/type_.rs).
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("id", &self.id)
            .field("instance", &self.instance)
            .field("type", &self.info().name)
            .field("common", &self.common)
            .finish_non_exhaustive()
    }
}

/// A source type, whichever it is.
pub trait ErasedStyleSourceType: Send + Sync + 'static {
    fn info(&self) -> SourceTypeInfo;

    /// The source `id` of [`StyleSourceType::from_json`]'s data.
    fn create_from_json(
        &self,
        id: &str,
        value: &Convertible<'_>,
        functions: &Arc<Functions>,
        error: &mut Error,
    ) -> Option<Source>;
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
    ) -> Option<Source> {
        T::from_json(value, functions, error).map(|data| Source::new(id, data))
    }
}

/// The source type `T` as a value.
#[must_use]
pub const fn style_source_entry<T: StyleSourceType>() -> &'static dyn ErasedStyleSourceType {
    SourceTypeEntry::<T>::INSTANCE
}
