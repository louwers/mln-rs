// Ports include/mln/style/sources/tile_source.hpp,
// src/mln/style/sources/tile_source.cpp,
// src/mln/style/sources/tile_source_impl.hpp and tile_source_impl.cpp, and
// the source classes on top of them: include/mln/style/sources/
// vector_source.hpp + vector_source.cpp, raster_source.hpp +
// raster_source.cpp.
//
// The C++ subclasses differ in `supportsLayerType` and
// `setTilesetOverrides`; here `TileSource` carries both as data (the
// TileJSON response handler applies the overrides) and `VectorSource` /
// `RasterSource` wrap it. Like GeoJSONSource, a TileSource is a handle to
// an `Rc`'d state (see source.rs).
//
// TODO(port): `util::mapbox::canonicalizeSourceURL` / `canonicalizeTileset`
// (only change URLs with the tile server's scheme alias, e.g. mapbox://).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_storage::{AsyncRequest, FileSource, Resource, Response};
use mln_style_types::SourceType;
use mln_util::constants;
use mln_util::immutable::Immutable;
use mln_util::tileset::{EmptyResponse, RasterEncoding, Tileset, VectorEncoding};

use crate::conversion::Error;
use crate::conversion::tileset::convert_json_tileset;
use crate::layer::LayerTypeInfo;
use crate::layer::layer_type_info::TileKind;
use crate::source::{Source, SourceBase, static_immutable_cast};
use crate::source_impl::{self, ImplBase};

/// `TileSource::Impl`.
#[derive(Clone, Debug)]
pub struct Impl {
    base: ImplBase,
    pub tileset: Option<Tileset>,
    tile_size: u16,
}

impl Impl {
    /// `Impl(SourceType, std::string id, uint16_t tileSize)`.
    #[must_use]
    pub fn new(source_type: SourceType, id: String, tile_size: u16) -> Self {
        Impl { base: ImplBase::new(source_type, id), tileset: None, tile_size }
    }

    /// `Impl(const Impl&, Tileset)`.
    #[must_use]
    pub fn with_tileset(other: &Impl, tileset: Tileset) -> Self {
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

/// `variant<std::string, Tileset>`.
#[derive(Clone, Debug, PartialEq)]
pub enum UrlOrTileset {
    Url(String),
    Tileset(Tileset),
}

/// The C++ subclass a `TileSource` is (observers get the most derived type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Subclass {
    Vector(VectorEncoding),
    Raster,
    // dem-heatmap
    RasterDEM,
}

/// What the C++ subclasses override.
#[derive(Clone, Copy, Debug)]
struct Overrides {
    subclass: Subclass,
    /// `supportsLayerType`: the tile kind of the layers the source serves.
    tile_kind: TileKind,
    /// `VectorSource`'s `maxZoom` / `minZoom`, or `RasterSource`'s `maxzoomOverride` /
    /// `minzoomOverride` (`setTilesetOverrides`).
    max_zoom: Option<f32>,
    min_zoom: Option<f32>,
    // dem-heatmap: RasterDEMSource's `options->rasterEncoding`.
    raster_encoding: Option<RasterEncoding>,
}

struct Inner {
    base: SourceBase,
    url_or_tileset: UrlOrTileset,
    req: RefCell<Option<Box<dyn AsyncRequest>>>,
    overrides: Overrides,
    /// The style's `emptyResponse`, which the source's tileset takes.
    empty_response: Cell<EmptyResponse>,
}

/// `TileSource`.
#[derive(Clone)]
pub struct TileSource {
    inner: Rc<Inner>,
}

impl std::fmt::Debug for TileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileSource")
            .field("base", &self.inner.base)
            .field("url_or_tileset", &self.inner.url_or_tileset)
            .finish_non_exhaustive()
    }
}

impl TileSource {
    /// `TileSource(id, urlOrTileset, tileSize, sourceType)`.
    fn new(
        id: String,
        url_or_tileset: UrlOrTileset,
        tile_size: u16,
        source_type: SourceType,
        overrides: Overrides,
    ) -> Self {
        let impl_: Immutable<dyn source_impl::Impl> = Arc::new(Impl::new(source_type, id, tile_size));
        TileSource {
            inner: Rc::new(Inner {
                base: SourceBase::new(impl_),
                url_or_tileset,
                req: RefCell::new(None),
                overrides,
                empty_response: Cell::new(EmptyResponse::Blank),
            }),
        }
    }

    /// `impl()`.
    #[must_use]
    pub fn impl_(&self) -> Arc<Impl> {
        static_immutable_cast::<Impl>(&self.inner.base.base_impl())
    }

    #[must_use]
    pub fn get_url_or_tileset(&self) -> &UrlOrTileset {
        &self.inner.url_or_tileset
    }

    #[must_use]
    pub fn get_url(&self) -> Option<String> {
        match &self.inner.url_or_tileset {
            UrlOrTileset::Url(url) => Some(url.clone()),
            UrlOrTileset::Tileset(_) => None,
        }
    }

    #[must_use]
    pub fn get_tile_size(&self) -> u16 {
        self.impl_().get_tile_size()
    }

    /// Sets the style's `emptyResponse`, which the tileset takes when the source loads its
    /// description.
    pub(crate) fn set_empty_response(&self, empty_response: EmptyResponse) {
        self.inner.empty_response.set(empty_response);
    }

    /// `setTilesetOverrides(Tileset&)`: `VectorSource`'s and `RasterSource`'s zoom
    /// overrides; the base class has none.
    fn set_tileset_overrides(&self, tileset: &mut Tileset) {
        let overrides = &self.inner.overrides;
        if let Some(max_zoom) = overrides.max_zoom {
            tileset.zoom_range.max = max_zoom as u8;
        }
        if let Some(min_zoom) = overrides.min_zoom {
            tileset.zoom_range.min = min_zoom as u8;
        }
        // dem-heatmap: RasterDEMSource::setTilesetOverrides
        if let Some(encoding) = overrides.raster_encoding {
            tileset.raster_encoding = Some(encoding);
        }
        tileset.empty_response = self.inner.empty_response.get();
    }
}

/// Calls `f` with the source as its most derived type (C++ `*this` in a
/// `VectorSource` / `RasterSource` member).
fn with_derived(this: &TileSource, f: impl FnOnce(&dyn Source)) {
    match this.inner.overrides.subclass {
        Subclass::Vector(encoding) => f(&VectorSource { tile_source: this.clone(), encoding }),
        Subclass::Raster => f(&RasterSource { tile_source: this.clone() }),
        // dem-heatmap
        Subclass::RasterDEM => f(&RasterDEMSource { tile_source: this.clone() }),
    }
}

/// `loadDescription`'s response handler.
fn on_description_response(weak: &Weak<Inner>, res: &Response) {
    let Some(inner) = weak.upgrade() else {
        return;
    };
    let this = TileSource { inner };
    with_derived(&this, |derived| on_description_response_for(&this, derived, res));
}

fn on_description_response_for(this: &TileSource, derived: &dyn Source, res: &Response) {
    let observer = this.inner.base.observer();
    let this_source = derived;
    if let Some(error) = &res.error {
        observer.on_source_error(this_source, mln_util::Error::native(error.message.clone()));
    } else if res.not_modified {
        // return
    } else if res.no_content {
        observer.on_source_error(this_source, mln_util::Error::native("unexpectedly empty TileJSON"));
    } else {
        let mut error = Error::default();
        let data = res.data.clone().unwrap_or_default();
        let Some(mut tileset) = convert_json_tileset(&data, &mut error) else {
            observer.on_source_error(this_source, mln_util::Error::native(error.message));
            return;
        };
        // TODO(port): canonicalizeTileset for the "mapbox" scheme alias.

        this.set_tileset_overrides(&mut tileset);

        let current = this.impl_();
        let changed = current.tileset.as_ref() != Some(&tileset);

        let new_impl: Immutable<dyn source_impl::Impl> = Arc::new(Impl::with_tileset(&current, tileset));
        this.inner.base.set_base_impl(new_impl);
        this.inner.base.set_loaded(true);

        observer.on_source_loaded(this_source);

        if changed {
            observer.on_source_changed(this_source);
        }
    }
}

impl Source for TileSource {
    fn base(&self) -> &SourceBase {
        &self.inner.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn load_description(&self, file_source: &dyn FileSource) {
        let raw_url = match &self.inner.url_or_tileset {
            UrlOrTileset::Tileset(tileset) => {
                let tileset = Tileset { empty_response: self.inner.empty_response.get(), ..tileset.clone() };
                let new_impl: Immutable<dyn source_impl::Impl> = Arc::new(Impl::with_tileset(&self.impl_(), tileset));
                self.inner.base.set_base_impl(new_impl);
                self.inner.base.set_loaded(true);
                let observer = self.inner.base.observer();
                with_derived(self, |derived| observer.on_source_loaded(derived));
                return;
            }
            UrlOrTileset::Url(url) => url.clone(),
        };

        if self.inner.req.borrow().is_some() {
            return;
        }

        // TODO(port): util::mapbox::canonicalizeSourceURL(tileServerOptions,
        // rawURL).
        let url = raw_url;

        let weak = Rc::downgrade(&self.inner);
        let req = file_source
            .request(&Resource::source(&url), Box::new(move |res: Response| on_description_response(&weak, &res)));
        *self.inner.req.borrow_mut() = Some(req);
    }

    fn supports_layer_type(&self, info: &LayerTypeInfo) -> bool {
        self.inner.overrides.tile_kind as u8 == info.tile_kind as u8
    }

    fn create_mutable(&self) -> Box<dyn source_impl::Impl> {
        Box::new((*self.impl_()).clone())
    }
}

/// `style::VectorSource`.
#[derive(Clone, Debug)]
pub struct VectorSource {
    tile_source: TileSource,
    encoding: VectorEncoding,
}

impl VectorSource {
    /// `VectorSource(id, urlOrTileset, maxZoom, minZoom, encoding)`.
    pub fn new(
        id: impl Into<String>,
        url_or_tileset: UrlOrTileset,
        max_zoom: Option<f32>,
        min_zoom: Option<f32>,
        encoding: VectorEncoding,
    ) -> Self {
        VectorSource {
            tile_source: TileSource::new(
                id.into(),
                url_or_tileset,
                constants::TILE_SIZE_I,
                SourceType::Vector,
                Overrides {
                    subclass: Subclass::Vector(encoding),
                    tile_kind: TileKind::Geometry,
                    max_zoom,
                    min_zoom,
                    raster_encoding: None,
                },
            ),
            encoding,
        }
    }

    #[must_use]
    pub fn tile_source(&self) -> &TileSource {
        &self.tile_source
    }

    /// Gets the tile urls for this vector source.
    #[must_use]
    pub fn get_tiles(&self) -> Vec<String> {
        match &self.tile_source.impl_().tileset {
            Some(tileset) => tileset.tiles.clone(),
            None => Vec::new(),
        }
    }

    /// Sets the tile urls for this vector source.
    pub fn set_tiles(&self, tiles: &[String]) {
        let impl_ = self.tile_source.impl_();
        let Some(tileset) = &impl_.tileset else {
            return;
        };
        if tileset.tiles == tiles {
            return;
        }
        let mut new_tileset = tileset.clone();
        new_tileset.tiles = tiles.to_vec();
        let new_impl: Immutable<dyn source_impl::Impl> = Arc::new(Impl::with_tileset(&impl_, new_tileset));
        self.tile_source.inner.base.set_base_impl(new_impl);
        self.tile_source.inner.base.observer().on_source_changed(self);
    }

    #[must_use]
    pub fn get_encoding(&self) -> VectorEncoding {
        self.encoding
    }
}

impl Source for VectorSource {
    fn base(&self) -> &SourceBase {
        self.tile_source.base()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn load_description(&self, file_source: &dyn FileSource) {
        self.tile_source.load_description(file_source);
    }

    fn supports_layer_type(&self, info: &LayerTypeInfo) -> bool {
        self.tile_source.supports_layer_type(info)
    }

    fn create_mutable(&self) -> Box<dyn source_impl::Impl> {
        self.tile_source.create_mutable()
    }
}

/// `style::RasterSource`.
#[derive(Clone, Debug)]
pub struct RasterSource {
    tile_source: TileSource,
}

impl RasterSource {
    /// `RasterSource(id, urlOrTileset, tileSize, sourceType = Raster, minzoom, maxzoom)`.
    /// `minzoom` / `maxzoom` are the style's inline values: they win over the TileJSON's, as in
    /// maplibre-gl-js.
    pub fn new(
        id: impl Into<String>,
        url_or_tileset: UrlOrTileset,
        tile_size: u16,
        source_type: SourceType,
        minzoom: Option<u8>,
        maxzoom: Option<u8>,
    ) -> Self {
        RasterSource {
            tile_source: TileSource::new(
                id.into(),
                url_or_tileset,
                tile_size,
                source_type,
                Overrides {
                    subclass: Subclass::Raster,
                    tile_kind: TileKind::Raster,
                    max_zoom: maxzoom.map(f32::from),
                    min_zoom: minzoom.map(f32::from),
                    raster_encoding: None,
                },
            ),
        }
    }

    #[must_use]
    pub fn tile_source(&self) -> &TileSource {
        &self.tile_source
    }
}

impl Source for RasterSource {
    fn base(&self) -> &SourceBase {
        self.tile_source.base()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn load_description(&self, file_source: &dyn FileSource) {
        self.tile_source.load_description(file_source);
    }

    fn supports_layer_type(&self, info: &LayerTypeInfo) -> bool {
        self.tile_source.supports_layer_type(info)
    }

    fn create_mutable(&self) -> Box<dyn source_impl::Impl> {
        self.tile_source.create_mutable()
    }
}

// dem-heatmap: ports include/mln/style/sources/raster_dem_source.hpp and
// src/mln/style/sources/raster_dem_source.cpp.
/// `style::RasterDEMSource` (a `RasterSource` serving `TileKind::RasterDEM`).
#[derive(Clone, Debug)]
pub struct RasterDEMSource {
    tile_source: TileSource,
}

impl RasterDEMSource {
    /// `RasterDEMSource(id, urlOrTileset, tileSize, options)`.
    pub fn new(
        id: impl Into<String>,
        url_or_tileset: UrlOrTileset,
        tile_size: u16,
        options: Option<crate::conversion::tileset::SourceOptions>,
    ) -> Self {
        RasterDEMSource {
            tile_source: TileSource::new(
                id.into(),
                url_or_tileset,
                tile_size,
                SourceType::RasterDEM,
                Overrides {
                    subclass: Subclass::RasterDEM,
                    tile_kind: TileKind::RasterDEM,
                    max_zoom: options.and_then(|options| options.maxzoom).map(f32::from),
                    min_zoom: options.and_then(|options| options.minzoom).map(f32::from),
                    raster_encoding: options.and_then(|options| options.raster_encoding),
                },
            ),
        }
    }

    #[must_use]
    pub fn tile_source(&self) -> &TileSource {
        &self.tile_source
    }
}

impl Source for RasterDEMSource {
    fn base(&self) -> &SourceBase {
        self.tile_source.base()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn load_description(&self, file_source: &dyn FileSource) {
        self.tile_source.load_description(file_source);
    }

    fn supports_layer_type(&self, info: &LayerTypeInfo) -> bool {
        self.tile_source.supports_layer_type(info)
    }

    fn create_mutable(&self) -> Box<dyn source_impl::Impl> {
        self.tile_source.create_mutable()
    }
}
