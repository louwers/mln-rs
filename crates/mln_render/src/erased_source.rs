//! The source type contract erased: the registry holds each source type as
//! `&'static dyn ErasedSourceType` ([`entry`]), and the tile store each
//! source's data origin as `Box<dyn ErasedOrigin>`, whose tiles keep their
//! loads as `Box<dyn TileLoad>`.

use std::any::Any;
use std::marker::PhantomData;

use mln_style::Source;
use mln_style::source_type::{ErasedStyleSourceType, SourceTypeEntry};

use crate::source::{Cover, DataOrigin, LoadContext, NewTile, OriginUpdate, SourceType, Supply, TileLoad};
use crate::tile::source_tiles::SourceTiles;
use crate::tile::store::StoreContext;
use crate::tile::tile::TileNecessity;
use crate::tile::tile_mut::TileMut;
use crate::tile_parameters::TileParameters;

/// A source type, whichever it is, that the renderer draws.
pub(crate) trait ErasedSourceType: ErasedStyleSourceType {
    /// [`SourceType::supply`].
    fn supply(&self, source: &Source) -> Supply;
}

impl<T: SourceType> ErasedSourceType for SourceTypeEntry<T> {
    fn supply(&self, source: &Source) -> Supply {
        T::supply(source)
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
    pub(crate) parameters: &'a TileParameters<'a>,
    pub(crate) needs_relayout: bool,
}

/// A data origin, whichever its type. Each method is the [`DataOrigin`]
/// hook of its name, with the tile's load downcast to the origin's.
pub(crate) trait ErasedOrigin {
    fn update(&mut self, parts: UpdateParts<'_>) -> Option<Cover>;
    fn new_load(&mut self, tile: &NewTile<'_>) -> Option<Box<dyn TileLoad>>;
    fn start(&mut self, load: &mut dyn TileLoad, tile: &mut TileMut<'_>);
    fn receive(&mut self, load: &mut dyn TileLoad, reply: Box<dyn Any + Send>, tile: &mut TileMut<'_>);
    fn set_necessity(&mut self, load: &mut dyn TileLoad, previous: TileNecessity, tile: &mut TileMut<'_>);
    fn update_parameters_changed(&mut self, load: &mut dyn TileLoad, tile: &mut TileMut<'_>);
    fn retry(&mut self, load: &mut dyn TileLoad, tile: &mut TileMut<'_>);
    fn take_world_copy(&self, load: &mut dyn TileLoad, from: &dyn TileLoad) -> bool;
}

/// `load` as a load of `O`'s, which every tile of a source has.
pub(crate) fn typed<O: DataOrigin>(load: &mut dyn TileLoad) -> Option<&mut O::Load> {
    let load: &mut dyn Any = load;
    let typed = load.downcast_mut();
    debug_assert!(typed.is_some(), "a tile's load is of its source's origin");
    typed
}

impl<O: DataOrigin> ErasedOrigin for O {
    fn update(&mut self, parts: UpdateParts<'_>) -> Option<Cover> {
        DataOrigin::update(self, &mut OriginUpdate { parts, origin: PhantomData })
    }

    fn new_load(&mut self, tile: &NewTile<'_>) -> Option<Box<dyn TileLoad>> {
        let load = DataOrigin::new_load(self, tile)?;
        Some(Box::new(load))
    }

    fn start(&mut self, load: &mut dyn TileLoad, tile: &mut TileMut<'_>) {
        if let Some(load) = typed::<O>(load) {
            DataOrigin::start(self, load, &mut LoadContext::new(tile));
        }
    }

    fn receive(&mut self, load: &mut dyn TileLoad, reply: Box<dyn Any + Send>, tile: &mut TileMut<'_>) {
        // A replier sends only its load's replies, of its origin's type.
        if let (Some(load), Ok(reply)) = (typed::<O>(load), reply.downcast::<O::Reply>()) {
            DataOrigin::receive(self, load, *reply, &mut LoadContext::new(tile));
        }
    }

    fn set_necessity(&mut self, load: &mut dyn TileLoad, previous: TileNecessity, tile: &mut TileMut<'_>) {
        if let Some(load) = typed::<O>(load) {
            DataOrigin::set_necessity(self, load, previous, &mut LoadContext::new(tile));
        }
    }

    fn update_parameters_changed(&mut self, load: &mut dyn TileLoad, tile: &mut TileMut<'_>) {
        if let Some(load) = typed::<O>(load) {
            DataOrigin::update_parameters_changed(self, load, &mut LoadContext::new(tile));
        }
    }

    fn retry(&mut self, load: &mut dyn TileLoad, tile: &mut TileMut<'_>) {
        if let Some(load) = typed::<O>(load) {
            DataOrigin::retry(self, load, &mut LoadContext::new(tile));
        }
    }

    fn take_world_copy(&self, load: &mut dyn TileLoad, from: &dyn TileLoad) -> bool {
        let from: &dyn Any = from;
        match (typed::<O>(load), from.downcast_ref()) {
            (Some(load), Some(from)) => DataOrigin::take_world_copy(self, load, from),
            _ => false,
        }
    }
}
