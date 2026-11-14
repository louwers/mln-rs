// One type serves C++'s VectorSource, RasterSource and RasterDEMSource, which
// differ only in their source type. Like GeoJSONSource, a TileSource is a
// handle to an `Rc`'d state (see source.rs).

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_storage::{AsyncRequest, FileSource, Resource, Response};
use mln_style_types::SourceType;
use mln_util::constants;
use mln_util::tileset::{PartialTileset, Tileset, VectorEncoding};

use crate::conversion::Convertible;
use crate::conversion::tileset::convert_tileset;
use crate::layer::LayerTypeInfo;
use crate::layer::layer_type_info::TileKind;
use crate::source::{Source, SourceBase, downcast_arc};
use crate::source_impl::{self, ImplBase};

#[derive(Clone, Debug)]
pub struct Impl {
    base: ImplBase,
    pub tileset: Option<Tileset>,
    tile_size: u16,
}

impl Impl {
    #[must_use]
    pub(crate) fn new(source_type: SourceType, id: String, tile_size: u16) -> Self {
        Impl { base: ImplBase::new(source_type, id), tileset: None, tile_size }
    }

    /// `Impl(const Impl&, Tileset)`.
    #[must_use]
    pub(crate) fn with_tileset(other: &Impl, tileset: Tileset) -> Self {
        Impl { base: other.base.clone(), tileset: Some(tileset), tile_size: other.tile_size }
    }

    #[must_use]
    pub fn get_tile_size(&self) -> u16 {
        self.tile_size
    }
}

impl source_impl::Impl for Impl {
    fn base(&self) -> &ImplBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ImplBase {
        &mut self.base
    }

    fn get_attribution(&self) -> Option<String> {
        self.tileset.as_ref().map(|tileset| tileset.attribution.clone())
    }
}

struct Inner {
    base: SourceBase,
    /// The URL of the source's TileJSON.
    url: Option<String>,
    /// The style's tileset members, which win over those of the TileJSON at `url`.
    style_tileset: PartialTileset,
    req: RefCell<Option<Box<dyn AsyncRequest>>>,
}

/// A vector, raster or raster-dem source: the TileJSON at its URL, the tileset members the style
/// lists, or both, the style's members winning.
#[derive(Clone)]
pub struct TileSource {
    inner: Rc<Inner>,
}

impl std::fmt::Debug for TileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileSource")
            .field("base", &self.inner.base)
            .field("url", &self.inner.url)
            .field("style_tileset", &self.inner.style_tileset)
            .finish_non_exhaustive()
    }
}

impl TileSource {
    #[must_use]
    pub fn vector(id: impl Into<String>, url: Option<String>, tileset: PartialTileset) -> Self {
        Self::new(id.into(), SourceType::Vector, url, tileset, constants::TILE_SIZE_I)
    }

    #[must_use]
    pub fn raster(id: impl Into<String>, url: Option<String>, tileset: PartialTileset, tile_size: u16) -> Self {
        Self::new(id.into(), SourceType::Raster, url, tileset, tile_size)
    }

    #[must_use]
    pub fn raster_dem(id: impl Into<String>, url: Option<String>, tileset: PartialTileset, tile_size: u16) -> Self {
        Self::new(id.into(), SourceType::RasterDEM, url, tileset, tile_size)
    }

    fn new(
        id: String,
        source_type: SourceType,
        url: Option<String>,
        style_tileset: PartialTileset,
        tile_size: u16,
    ) -> Self {
        let impl_: Arc<dyn source_impl::Impl> = Arc::new(Impl::new(source_type, id, tile_size));
        TileSource {
            inner: Rc::new(Inner { base: SourceBase::new(impl_), url, style_tileset, req: RefCell::new(None) }),
        }
    }

    #[must_use]
    pub(crate) fn impl_(&self) -> Arc<Impl> {
        downcast_arc::<Impl>(&self.inner.base.base_impl())
    }

    #[must_use]
    pub fn get_url(&self) -> Option<String> {
        self.inner.url.clone()
    }

    #[must_use]
    pub fn get_tile_size(&self) -> u16 {
        self.impl_().get_tile_size()
    }

    /// The tileset the style lists, for a source without a URL.
    #[must_use]
    pub fn inline_tileset(&self) -> Option<Tileset> {
        self.inner.url.is_none().then(|| self.inner.style_tileset.clone().into())
    }

    /// The source's tileset once `tilejson`, the TileJSON at its URL, loads: the style's members
    /// over the document's.
    ///
    /// # Errors
    ///
    /// When `tilejson` isn't a valid TileJSON, or neither it nor the style lists tiles.
    pub fn tileset_with_tilejson(&self, tilejson: &[u8]) -> Result<Tileset, String> {
        let document: serde_json::Value = serde_json::from_slice(tilejson).map_err(|e| e.to_string())?;
        let document = convert_tileset(&Convertible::from(&document), self.get_type())?;
        let tileset = self.inner.style_tileset.clone().or(document);
        if tileset.tiles.is_none() {
            return Err("source must have tiles".to_owned());
        }
        Ok(tileset.into())
    }

    /// Sets the tileset that `load_description` found and marks the source loaded. Returns
    /// whether the tileset changed.
    fn set_tileset(&self, tileset: Tileset) -> bool {
        let current = self.impl_();
        let changed = current.tileset.as_ref() != Some(&tileset);
        let new_impl: Arc<dyn source_impl::Impl> = Arc::new(Impl::with_tileset(&current, tileset));
        self.inner.base.set_base_impl(new_impl);
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

    fn load_description(&self, file_source: &dyn FileSource) {
        let Some(url) = &self.inner.url else {
            self.set_tileset(self.inner.style_tileset.clone().into());
            self.inner.base.observer().on_source_loaded(self);
            return;
        };
        if self.inner.req.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(&self.inner);
        let req = file_source
            .request(&Resource::source(url), Box::new(move |res: Response| on_description_response(&weak, &res)));
        *self.inner.req.borrow_mut() = Some(req);
    }

    fn supports_layer_type(&self, info: &LayerTypeInfo) -> bool {
        let tile_kind = match self.get_type() {
            SourceType::Vector => TileKind::Geometry,
            SourceType::Raster => TileKind::Raster,
            _ => TileKind::RasterDEM,
        };
        tile_kind as u8 == info.tile_kind as u8
    }

    fn create_mutable(&self) -> Box<dyn source_impl::Impl> {
        Box::new((*self.impl_()).clone())
    }
}

impl TileSource {
    /// The source's tile URLs.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn get_tiles(&self) -> Vec<String> {
        self.impl_().tileset.as_ref().map(|tileset| tileset.tiles.clone()).unwrap_or_default()
    }

    /// Sets the tile URLs of the source's loaded tileset.
    pub fn set_tiles(&self, tiles: &[String]) {
        let impl_ = self.impl_();
        let Some(tileset) = &impl_.tileset else {
            return;
        };
        if tileset.tiles == tiles {
            return;
        }
        let tileset = Tileset { tiles: tiles.to_vec(), ..tileset.clone() };
        let new_impl: Arc<dyn source_impl::Impl> = Arc::new(Impl::with_tileset(&impl_, tileset));
        self.inner.base.set_base_impl(new_impl);
        self.inner.base.observer().on_source_changed(self);
    }

    /// The encoding of a vector source's tiles: its tileset's once it's loaded, else the style's.
    #[must_use]
    pub fn get_vector_encoding(&self) -> VectorEncoding {
        let loaded = self.impl_().tileset.as_ref().and_then(|tileset| tileset.vector_encoding);
        loaded.or(self.inner.style_tileset.vector_encoding).unwrap_or(VectorEncoding::Mapbox)
    }
}
