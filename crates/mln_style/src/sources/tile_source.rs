// A TileSource is a handle to an `Rc`'d state (see source.rs), and serves
// the vector, raster and raster-dem source types alike.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use mln_storage::{FileSource, FileSourceExt, Request, Resource, Response};
use mln_util::constants;
use mln_util::tileset::{PartialTileset, Tileset, VectorEncoding};

use super::tileset::{
    RasterDemSourceType, RasterSourceType, TilesetSourceOptions, VectorSourceType, tileset_parts, with_tileset,
};
use crate::source::{Source, SourceBase};
use crate::source_type::{self, SourceData, StyleSourceType};

struct Inner {
    base: SourceBase,
    req: RefCell<Option<Request>>,
}

/// A vector, raster or raster-dem source: the TileJSON at its URL, the tileset members the style
/// lists, or both, the style's members winning.
#[derive(Clone)]
pub struct TileSource {
    inner: Rc<Inner>,
}

impl std::fmt::Debug for TileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileSource").field("base", &self.inner.base).finish_non_exhaustive()
    }
}

impl TileSource {
    #[must_use]
    pub fn vector(id: impl Into<String>, url: Option<String>, tileset: PartialTileset) -> Self {
        Self::of::<VectorSourceType>(&id.into(), url, tileset, constants::TILE_SIZE_I)
    }

    #[must_use]
    pub fn raster(id: impl Into<String>, url: Option<String>, tileset: PartialTileset, tile_size: u16) -> Self {
        Self::of::<RasterSourceType>(&id.into(), url, tileset, tile_size)
    }

    #[must_use]
    pub fn raster_dem(id: impl Into<String>, url: Option<String>, tileset: PartialTileset, tile_size: u16) -> Self {
        Self::of::<RasterDemSourceType>(&id.into(), url, tileset, tile_size)
    }

    fn of<T: StyleSourceType<Options = TilesetSourceOptions>>(
        id: &str,
        url: Option<String>,
        tileset: PartialTileset,
        tile_size: u16,
    ) -> Self {
        let options = TilesetSourceOptions { url, tileset, tile_size };
        Self::new(source_type::Source::new(id, SourceData::<T>::new(options)))
    }

    /// The handle of `record`, a tileset source's.
    pub(crate) fn new(record: source_type::Source) -> Self {
        TileSource { inner: Rc::new(Inner { base: SourceBase::new(record), req: RefCell::new(None) }) }
    }

    fn options(&self) -> Option<TilesetSourceOptions> {
        let record = self.inner.base.base_impl();
        tileset_parts(&record).map(|(options, _)| options.clone())
    }

    fn tileset(&self) -> Option<Tileset> {
        let record = self.inner.base.base_impl();
        tileset_parts(&record).and_then(|(_, tileset)| tileset.cloned())
    }

    #[must_use]
    pub fn get_url(&self) -> Option<String> {
        self.options().and_then(|options| options.url)
    }

    #[must_use]
    pub fn get_tile_size(&self) -> u16 {
        self.options().map_or(constants::TILE_SIZE_I, |options| options.tile_size)
    }

    /// The tileset the style lists, for a source without a URL.
    #[must_use]
    pub fn inline_tileset(&self) -> Option<Tileset> {
        self.options().and_then(|options| options.inline_tileset())
    }

    /// The source's tileset once `tilejson`, the TileJSON at its URL, loads: the style's members
    /// over the document's.
    ///
    /// # Errors
    ///
    /// When `tilejson` isn't a valid TileJSON, or neither it nor the style lists tiles.
    pub fn tileset_with_tilejson(&self, tilejson: &[u8]) -> Result<Tileset, String> {
        let options = self.options().ok_or_else(|| "not a tileset source".to_owned())?;
        options.tileset_with_tilejson(tilejson, self.get_type())
    }

    /// Sets the tileset that `load_description` found and marks the source loaded. Returns
    /// whether the tileset changed.
    fn set_tileset(&self, tileset: Tileset) -> bool {
        let record = self.inner.base.base_impl();
        let changed = self.tileset().as_ref() != Some(&tileset);
        if let Some(record) = with_tileset(&record, tileset) {
            self.inner.base.set_base_impl(record);
        }
        self.inner.base.set_loaded(true);
        changed
    }
}

/// `loadDescription`'s response handler.
fn on_description_response(weak: &Weak<Inner>, res: &Response) {
    let Some(inner) = weak.upgrade() else {
        return;
    };
    let this = TileSource { inner };
    let observer = this.inner.base.observer();
    let tileset = if let Some(error) = &res.error {
        Err(error.message.clone())
    } else if res.not_modified {
        return;
    } else if res.no_content {
        Err("unexpectedly empty TileJSON".to_owned())
    } else {
        this.tileset_with_tilejson(res.data.as_deref().map_or(&[][..], Vec::as_slice))
    };
    match tileset {
        Ok(tileset) => {
            let changed = this.set_tileset(tileset);
            observer.on_source_loaded(&this);
            if changed {
                observer.on_source_changed(&this);
            }
        }
        Err(message) => observer.on_source_error(&this, mln_util::Error::native(message)),
    }
}

impl Source for TileSource {
    fn base(&self) -> &SourceBase {
        &self.inner.base
    }

    fn load_description(&self, file_source: &dyn FileSource, run_loop: &mln_actor::RunLoop) {
        let Some(options) = self.options() else {
            return;
        };
        let Some(url) = &options.url else {
            self.set_tileset(options.tileset.into());
            self.inner.base.observer().on_source_loaded(self);
            return;
        };
        if self.inner.req.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(&self.inner);
        let req = file_source.request(
            &run_loop.scheduler(),
            &Resource::source(url),
            Box::new(move |res: Response| on_description_response(&weak, &res)),
        );
        *self.inner.req.borrow_mut() = Some(req);
    }
}

impl TileSource {
    /// The source's tile URLs.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn get_tiles(&self) -> Vec<String> {
        self.tileset().map(|tileset| tileset.tiles).unwrap_or_default()
    }

    /// Sets the tile URLs of the source's loaded tileset.
    pub fn set_tiles(&self, tiles: &[String]) {
        let Some(tileset) = self.tileset() else {
            return;
        };
        if tileset.tiles == tiles {
            return;
        }
        let tileset = Tileset { tiles: tiles.to_vec(), ..tileset };
        if let Some(record) = with_tileset(&self.inner.base.base_impl(), tileset) {
            self.inner.base.set_base_impl(record);
        }
        self.inner.base.observer().on_source_changed(self);
    }

    /// The encoding of a vector source's tiles: its tileset's once it's loaded, else the style's.
    #[must_use]
    pub fn get_vector_encoding(&self) -> VectorEncoding {
        let loaded = self.tileset().and_then(|tileset| tileset.vector_encoding);
        let style = self.options().and_then(|options| options.tileset.vector_encoding);
        loaded.or(style).unwrap_or(VectorEncoding::Mapbox)
    }
}
