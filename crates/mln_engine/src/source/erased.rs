//! The source type contract erased: the registry holds each source type as
//! `&'static dyn ErasedSourceType` ([`entry`]), and the tile store each
//! source's data origin as `Box<dyn ErasedOrigin>`, whose tiles keep the
//! origin's state of them as `Box<dyn Any>`.

use std::any::Any;
use std::marker::PhantomData;

use mln_style::Source;
use mln_style::layer::SourceKind;
use mln_style::source::{ErasedStyleSourceType, SourceTypeEntry};

use crate::source::{Cover, DataOrigin, LoadContext, MapSettings, OriginUpdate, SourceType, Supplied, WorldCopy, kind};
use crate::tile_store::source_tiles::SourceTiles;
use crate::tile_store::store::StoreContext;
use crate::tile_store::tile::TileNecessity;
use crate::tile_store::tile_mut::{CopyAnswer, TileMut};

/// A source type, whichever it is, that the renderer draws.
pub(crate) trait ErasedSourceType: ErasedStyleSourceType {
    /// [`SourceType::supply`].
    fn supply(&self, source: &Source) -> Supplied;
}

impl<T: SourceType> ErasedSourceType for SourceTypeEntry<T> {
    fn supply(&self, source: &Source) -> Supplied {
        T::supply(source).0
    }
}

/// The source type `T` as a value.
pub(crate) const fn entry<T: SourceType>() -> &'static dyn ErasedSourceType {
    SourceTypeEntry::<T>::INSTANCE
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
