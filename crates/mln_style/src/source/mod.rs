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
//! tileset of a TileJSON. This crate holds it without knowing its type, as an
//! [`ErasedDescription`]: the engine's half of the source type says how it
//! loads, and the map's loader applies what a load finds as an
//! [`Edit::SetSourceDescription`](crate::Edit).

use std::any::Any;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::layer::SourceKind;
use crate::{constants, log_target};

mod convert;
mod list;

pub use convert::convert_source;
pub use list::SourceList;

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
}

/// A source's description, of the type its source type's loading half
/// names, which this crate holds without reading it.
pub type ErasedDescription = Arc<dyn Any + Send + Sync>;

/// A source type's style half: its name, what its sources hold and how style
/// JSON describes them.
pub trait StyleSourceType: Sized + Send + Sync + 'static {
    const INFO: SourceTypeInfo;

    /// What the style or the API gives: a URL, an inline tileset, GeoJSON
    /// options, coordinates.
    type Options: Clone + Send + Sync + 'static;

    /// The data of a source from its style JSON `value`, whose expressions
    /// parse against `functions`. Only a type whose [`INFO`](Self::INFO)
    /// has `from_json` is asked.
    fn from_json(_value: &Convertible<'_>, _functions: &Arc<Functions>, error: &mut Error) -> Option<SourceData<Self>> {
        error.message = "invalid source type".to_owned();
        None
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
    pub description: Option<ErasedDescription>,
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
    pub fn with_description(options: T::Options, description: ErasedDescription) -> Self {
        SourceData { options, description: Some(description), load: 0 }
    }

    /// `source`'s data as a `T` source's, or `None` for another type's.
    #[must_use]
    pub fn recover(source: &Source) -> Option<&Self> {
        let any: &dyn Any = &*source.data;
        any.downcast_ref()
    }

    /// The source's description as a `D`, or `None` until it loads or if
    /// it's another type.
    #[must_use]
    pub fn description<D: Any>(&self) -> Option<&D> {
        self.description.as_deref()?.downcast_ref()
    }
}

/// A source's data, whichever its type.
pub trait ErasedSource: Any + Send + Sync {
    fn source_type(&self) -> &'static dyn ErasedStyleSourceType;

    /// [`SourceData::load`].
    fn load(&self) -> u64;

    /// [`SourceData::description`].
    fn description(&self) -> Option<&ErasedDescription>;

    /// This data with `description` in place of its description.
    fn with_description(&self, description: Option<ErasedDescription>) -> Arc<dyn ErasedSource>;
}

impl<T: StyleSourceType> ErasedSource for SourceData<T> {
    fn source_type(&self) -> &'static dyn ErasedStyleSourceType {
        style_source_entry::<T>()
    }

    fn load(&self) -> u64 {
        self.load
    }

    fn description(&self) -> Option<&ErasedDescription> {
        self.description.as_ref()
    }

    fn with_description(&self, description: Option<ErasedDescription>) -> Arc<dyn ErasedSource> {
        Arc::new(SourceData::<T> { options: self.options.clone(), description, load: self.load })
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

    /// This source with `description` as its description.
    #[must_use]
    pub fn with_description(&self, description: Option<ErasedDescription>) -> Self {
        self.with_erased_data(self.data.with_description(description))
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
