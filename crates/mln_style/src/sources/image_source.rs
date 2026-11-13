// Like the other sources, an ImageSource is a handle to an `Rc`'d state
// (see source.rs); the image request callback holds a `Weak` to it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_image::decoder;
use mln_storage::{AsyncRequest, FileSource, Resource, Response};
use mln_style_types::SourceType;
use mln_util::LatLng;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;

use crate::layer::LayerTypeInfo;
use crate::layer::layer_type_info::TileKind;
use crate::source::{Source, SourceBase, downcast_arc};
use crate::source_impl::{self, ImplBase};

#[derive(Clone, Debug)]
pub struct Impl {
    base: ImplBase,
    coords: [LatLng; 4],
    image: Option<Arc<PremultipliedImage>>,
}

impl Impl {
    #[must_use]
    pub fn new(id: String, coords: [LatLng; 4]) -> Self {
        Impl { base: ImplBase::new(SourceType::Image, id), coords, image: None }
    }

    #[must_use]
    pub fn with_coordinates(other: &Impl, coords: [LatLng; 4]) -> Self {
        Impl { base: other.base.clone(), coords, image: other.image.clone() }
    }

    #[must_use]
    pub fn with_image(rhs: &Impl, image: PremultipliedImage) -> Self {
        Impl { base: rhs.base.clone(), coords: rhs.coords, image: Some(Arc::new(image)) }
    }

    #[must_use]
    pub fn get_image(&self) -> Option<Arc<PremultipliedImage>> {
        self.image.clone()
    }

    #[must_use]
    pub fn get_coordinates(&self) -> [LatLng; 4] {
        self.coords
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
        None
    }
}

struct Inner {
    base: SourceBase,
    url: RefCell<Option<String>>,
    req: RefCell<Option<Box<dyn AsyncRequest>>>,
    /// Port-only: counts the URLs and images set, so that an image that
    /// decodes after another was set (a decoder that answers later, see
    /// `on_image_response`) is dropped, as a cancelled request's response.
    generation: Cell<u64>,
}

#[derive(Clone)]
pub struct ImageSource {
    inner: Rc<Inner>,
}

impl std::fmt::Debug for ImageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageSource")
            .field("base", &self.inner.base)
            .field("url", &self.inner.url.borrow())
            .finish_non_exhaustive()
    }
}

impl ImageSource {
    pub fn new(id: impl Into<String>, coords: [LatLng; 4]) -> Self {
        let impl_: Arc<dyn source_impl::Impl> = Arc::new(Impl::new(id.into(), coords));
        ImageSource {
            inner: Rc::new(Inner {
                base: SourceBase::new(impl_),
                url: RefCell::new(None),
                req: RefCell::new(None),
                generation: Cell::new(0),
            }),
        }
    }

    #[must_use]
    pub fn impl_(&self) -> Arc<Impl> {
        downcast_arc::<Impl>(&self.inner.base.base_impl())
    }

    pub fn set_coordinates(&self, coords: [LatLng; 4]) {
        let new_impl: Arc<dyn source_impl::Impl> = Arc::new(Impl::with_coordinates(&self.impl_(), coords));
        self.inner.base.set_base_impl(new_impl);
        self.inner.base.observer().on_source_changed(self);
    }

    #[must_use]
    pub fn get_coordinates(&self) -> [LatLng; 4] {
        self.impl_().get_coordinates()
    }

    pub fn set_url(&self, url: &str) {
        *self.inner.url.borrow_mut() = Some(url.to_owned());
        self.inner.generation.set(self.inner.generation.get() + 1);
        // Signal that the source description needs a reload
        if self.inner.base.loaded() || self.inner.req.borrow().is_some() {
            self.inner.base.set_loaded(false);
            *self.inner.req.borrow_mut() = None;
            self.inner.base.observer().on_source_description_changed(self);
        }
    }

    pub fn set_image(&self, image: PremultipliedImage) {
        *self.inner.url.borrow_mut() = None;
        self.inner.generation.set(self.inner.generation.get() + 1);
        *self.inner.req.borrow_mut() = None;
        self.inner.base.set_loaded(true);
        let new_impl: Arc<dyn source_impl::Impl> = Arc::new(Impl::with_image(&self.impl_(), image));
        self.inner.base.set_base_impl(new_impl);
        self.inner.base.observer().on_source_changed(self);
    }

    #[must_use]
    pub fn get_url(&self) -> Option<String> {
        self.inner.url.borrow().clone()
    }
}

/// `loadDescription`'s response handler. The image decodes through
/// `mln_image`'s `decoder`, whose answer may come after this
/// returns (the browser's); it's dropped if the source is gone or got
/// another URL or image by then.
fn on_image_response(weak: &Weak<Inner>, res: &Response) {
    let Some(inner) = weak.upgrade() else {
        return;
    };
    let this = ImageSource { inner };
    let observer = this.inner.base.observer();
    if let Some(error) = &res.error {
        observer.on_source_error(&this, Error::native(error.message.clone()));
    } else if res.not_modified {
        // return
    } else if res.no_content {
        observer.on_source_error(&this, Error::native("unexpectedly empty image url"));
    } else {
        let data = res.data.clone().unwrap_or_default();
        let weak = weak.clone();
        let generation = this.inner.generation.get();
        decoder::decode(&data, move |result| {
            let Some(inner) = weak.upgrade().filter(|inner| inner.generation.get() == generation) else {
                return;
            };
            let this = ImageSource { inner };
            let observer = this.inner.base.observer();
            match result {
                Ok(image) => {
                    let new_impl: Arc<dyn source_impl::Impl> = Arc::new(Impl::with_image(&this.impl_(), image));
                    this.inner.base.set_base_impl(new_impl);
                }
                Err(error) => observer.on_source_error(&this, Error::native(error.message)),
            }
            this.inner.base.set_loaded(true);
            observer.on_source_loaded(&this);
        });
    }
}

impl Source for ImageSource {
    fn base(&self) -> &SourceBase {
        &self.inner.base
    }

    fn load_description(&self, file_source: &dyn FileSource) {
        let url = self.inner.url.borrow().clone();
        let Some(url) = url else {
            self.inner.base.set_loaded(true);
            return;
        };

        if self.inner.req.borrow().is_some() || self.inner.base.loaded() {
            return;
        }

        let weak = Rc::downgrade(&self.inner);
        let req =
            file_source.request(&Resource::image(&url), Box::new(move |res: Response| on_image_response(&weak, &res)));
        *self.inner.req.borrow_mut() = Some(req);
    }

    fn supports_layer_type(&self, info: &LayerTypeInfo) -> bool {
        TileKind::Raster as u8 == info.tile_kind as u8
    }

    fn create_mutable(&self) -> Box<dyn source_impl::Impl> {
        Box::new((*self.impl_()).clone())
    }
}
