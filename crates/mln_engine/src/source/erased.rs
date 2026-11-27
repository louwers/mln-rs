//! The source type contract erased: the registry holds each source type as
//! `&'static dyn ErasedSourceType` ([`entry`]), and the tile store each
//! source's data origin as `Box<dyn ErasedOrigin>`, whose tiles keep the
//! origin's state of them as `Box<dyn Any>`.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use mln_style::Source;
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::layer::SourceKind;
use mln_style::source::{ErasedDescription, ErasedStyleSourceType, SourceData, style_source_entry};
use mln_style::tileset::Tileset;

use crate::query::FeatureExtensionValue;
use crate::source::{
    Cover, DataOrigin, Described, Load, LoadContext, MapSettings, OfflineResources, OriginUpdate, Respond,
    SourceSummary, SourceType, Supplied, WorldCopy, kind,
};
use crate::storage::Data;
use crate::tile_store::source_tiles::SourceTiles;
use crate::tile_store::store::StoreContext;
use crate::tile_store::tile::TileNecessity;
use crate::tile_store::tile_mut::{CopyAnswer, TileMut};

/// A job that describes a source's fetched data, answering through the
/// [`Respond`] it takes.
pub(crate) type DescribeJob = Box<dyn FnOnce(Respond<Described<ErasedDescription>>) + Send>;

/// A job that describes a source's decoded image.
pub(crate) type DescribeImageJob = Box<dyn FnOnce(PremultipliedImage) -> Described<ErasedDescription> + Send>;

/// A source type, whichever it is. Each method that takes a source is the
/// [`SourceType`] hook of its name for that source, and does nothing for a
/// source of another type.
pub(crate) trait ErasedSourceType: Send + Sync + 'static {
    /// The style half, which a style parses against.
    fn style(&self) -> &'static dyn ErasedStyleSourceType;
    fn supply(&self, source: &Source) -> Supplied;
    fn plan_load(&self, source: &Source) -> Load<ErasedDescription>;
    fn describes_in_background(&self) -> bool;
    /// [`SourceType::describe`] of `data` for the source's options, as a job
    /// that owns what it needs and may run on another thread.
    fn describe_job(&self, source: &Source, data: Option<Data>) -> DescribeJob;
    /// [`SourceType::describe_image`] for the source's options, as a job
    /// that owns what it needs and may run on another thread.
    fn describe_image_job(&self, source: &Source) -> DescribeImageJob;
    /// [`SourceType::metadata_changed`] from `previous`'s description to
    /// `source`'s.
    fn metadata_changed(&self, previous: &Source, source: &Source) -> bool;
    fn attribution(&self, source: &Source) -> Option<String>;
    fn summary(&self, source: &Source) -> SourceSummary;
    #[cfg_attr(not(feature = "sqlite"), expect(dead_code, reason = "only offline downloads ask"))]
    fn offline(&self, source: &Source) -> OfflineResources;
    #[cfg_attr(not(feature = "sqlite"), expect(dead_code, reason = "only offline downloads ask"))]
    fn offline_tileset(&self, source: &Source, tilejson: &[u8]) -> Result<Tileset, String>;
    fn query_feature_extension(
        &self,
        source: &Source,
        feature: &crate::tile::feature::Feature,
        extension: &str,
        field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<FeatureExtensionValue, Error>;
}

/// The [`ErasedSourceType`] of `T`, which [`entry`] returns.
struct Entry<T>(PhantomData<fn() -> T>);

impl<T: 'static> Entry<T> {
    const INSTANCE: &'static Self = &Entry(PhantomData);
}

fn erase<D: Send + Sync + 'static>(description: D) -> ErasedDescription {
    Arc::new(description)
}

fn data<T: SourceType>(source: &Source) -> Option<&SourceData<T::Style>> {
    SourceData::<T::Style>::recover(source)
}

fn description<T: SourceType>(source: &Source) -> Option<&T::Description> {
    data::<T>(source)?.description::<T::Description>()
}

impl<T: SourceType> ErasedSourceType for Entry<T> {
    fn style(&self) -> &'static dyn ErasedStyleSourceType {
        style_source_entry::<T::Style>()
    }

    fn supply(&self, source: &Source) -> Supplied {
        T::supply(source).0
    }

    fn plan_load(&self, source: &Source) -> Load<ErasedDescription> {
        let Some(data) = data::<T>(source) else { return Load::Done };
        match T::load(data) {
            Load::Done => Load::Done,
            Load::Ready(description) => Load::Ready(erase(description)),
            Load::Describe => Load::Describe,
            Load::Fetch(fetch) => Load::Fetch(fetch),
        }
    }

    fn describes_in_background(&self) -> bool {
        T::DESCRIBES_IN_BACKGROUND
    }

    fn describe_job(&self, source: &Source, data: Option<Data>) -> DescribeJob {
        let options = self::data::<T>(source).map(|source| source.options.clone());
        Box::new(move |respond| {
            let Some(options) = options else {
                respond(Err(Error::native("a source of another type")));
                return;
            };
            T::describe(&options, data, Box::new(move |described| respond(described.map(|d| d.map(erase)))));
        })
    }

    fn describe_image_job(&self, source: &Source) -> DescribeImageJob {
        let options = data::<T>(source).map(|source| source.options.clone());
        Box::new(move |image| {
            let options = options.ok_or_else(|| Error::native("a source of another type"))?;
            T::describe_image(&options, image).map(|d| d.map(erase))
        })
    }

    fn metadata_changed(&self, previous: &Source, source: &Source) -> bool {
        data::<T>(previous).is_some()
            && data::<T>(source).is_some()
            && T::metadata_changed(description::<T>(previous), description::<T>(source))
    }

    fn attribution(&self, source: &Source) -> Option<String> {
        T::attribution(data::<T>(source)?)
    }

    fn summary(&self, source: &Source) -> SourceSummary {
        data::<T>(source).map(T::summary).unwrap_or_default()
    }

    fn offline(&self, source: &Source) -> OfflineResources {
        data::<T>(source).map(T::offline).unwrap_or_default()
    }

    fn offline_tileset(&self, source: &Source, tilejson: &[u8]) -> Result<Tileset, String> {
        let data = data::<T>(source).ok_or_else(|| "a source of another type".to_owned())?;
        T::offline_tileset(data, tilejson)
    }

    fn query_feature_extension(
        &self,
        source: &Source,
        feature: &crate::tile::feature::Feature,
        extension: &str,
        field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<FeatureExtensionValue, Error> {
        T::query_feature_extension(source, feature, extension, field, args)
    }
}

/// The source type `T` as a value.
pub(crate) const fn entry<T: SourceType>() -> &'static dyn ErasedSourceType {
    Entry::<T>::INSTANCE
}

/// What an origin's update takes in, whatever its type.
pub(crate) struct UpdateParts<'a> {
    pub(crate) source: &'a Source,
    pub(crate) tiles: &'a mut SourceTiles,
    pub(crate) ctx: &'a mut StoreContext,
    pub(crate) map: &'a MapSettings,
    pub(crate) needs_relayout: bool,
}

/// A data origin, whichever its type. Each method is the [`DataOrigin`]
/// hook of its name, with the origin's state of the tile downcast to the
/// origin's [`Tile`](DataOrigin::Tile).
pub(crate) trait ErasedOrigin {
    /// The kind of data the origin delivers, its [`Data`](DataOrigin::Data).
    fn kind(&self) -> SourceKind;
    fn update(&mut self, parts: UpdateParts<'_>) -> Option<Cover>;
    /// `copy` is the origin's state of the same tile in another world copy,
    /// and what taking that copy settles the new tile on.
    fn start(&mut self, copy: Option<(&dyn Any, Option<CopyAnswer>)>, tile: &mut TileMut<'_>) -> Box<dyn Any>;
    fn receive(&mut self, state: &mut dyn Any, reply: Box<dyn Any + Send>, tile: &mut TileMut<'_>);
    fn set_necessity(&mut self, state: &mut dyn Any, previous: TileNecessity, tile: &mut TileMut<'_>);
    fn retry(&mut self, state: &mut dyn Any, tile: &mut TileMut<'_>);
    fn failed(&mut self, state: &mut dyn Any, tile: &mut TileMut<'_>);
}

/// `state` as a tile's state of `O`'s, which every tile of a source has.
pub(crate) fn typed<O: DataOrigin>(state: &mut dyn Any) -> Option<&mut O::Tile> {
    let typed = state.downcast_mut();
    debug_assert!(typed.is_some(), "a tile's state is of its source's origin");
    typed
}

impl<O: DataOrigin> ErasedOrigin for O {
    fn kind(&self) -> SourceKind {
        <O::Data as kind::Deliver>::KIND
    }

    fn update(&mut self, parts: UpdateParts<'_>) -> Option<Cover> {
        DataOrigin::update(self, &mut OriginUpdate { parts, origin: PhantomData })
    }

    fn start(&mut self, copy: Option<(&dyn Any, Option<CopyAnswer>)>, tile: &mut TileMut<'_>) -> Box<dyn Any> {
        let copy = copy.and_then(|(state, answer)| Some(WorldCopy { state: state.downcast_ref()?, answer }));
        Box::new(DataOrigin::start(self, &mut LoadContext::<O>::new(tile), copy))
    }

    fn receive(&mut self, state: &mut dyn Any, reply: Box<dyn Any + Send>, tile: &mut TileMut<'_>) {
        // A replier sends only its load's replies, of its origin's type.
        if let (Some(state), Ok(reply)) = (typed::<O>(state), reply.downcast::<O::Reply>()) {
            DataOrigin::receive(self, state, *reply, &mut LoadContext::<O>::new(tile));
        }
    }

    fn set_necessity(&mut self, state: &mut dyn Any, previous: TileNecessity, tile: &mut TileMut<'_>) {
        if let Some(state) = typed::<O>(state) {
            DataOrigin::set_necessity(self, state, previous, &mut LoadContext::<O>::new(tile));
        }
    }

    fn retry(&mut self, state: &mut dyn Any, tile: &mut TileMut<'_>) {
        if let Some(state) = typed::<O>(state) {
            DataOrigin::retry(self, state, &mut LoadContext::<O>::new(tile));
        }
    }

    fn failed(&mut self, state: &mut dyn Any, tile: &mut TileMut<'_>) {
        if let Some(state) = typed::<O>(state) {
            DataOrigin::failed(self, state, &mut LoadContext::<O>::new(tile));
        }
    }
}
