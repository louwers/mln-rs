// Like the other sources, an ImageSource is a handle to an `Rc`'d state
// (see source.rs); the image request callback holds a `Weak` to it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_image::decoder;
use mln_storage::{FileSource, FileSourceExt, Request, Resource, Response};
use mln_util::LatLng;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;

use super::image::{ImageSourceOptions, ImageSourceType};
use crate::source::{Source, SourceBase};
use crate::source_type::{self, SourceData};

struct Inner {
    base: SourceBase,
    req: RefCell<Option<Request>>,
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
        f.debug_struct("ImageSource").field("base", &self.inner.base).finish_non_exhaustive()
    }
}

impl ImageSource {
    pub fn new(id: impl Into<String>, coords: [LatLng; 4]) -> Self {
        let data = SourceData::<ImageSourceType>::new(ImageSourceOptions { url: None, coordinates: coords });
        ImageSource {
            inner: Rc::new(Inner {
                base: SourceBase::new(source_type::Source::new(&id.into(), data)),
                req: RefCell::new(None),
                generation: Cell::new(0),
            }),
        }
    }

    fn read<R>(&self, f: impl FnOnce(&SourceData<ImageSourceType>) -> R) -> Option<R> {
        SourceData::recover(&self.inner.base.base_impl()).map(f)
    }

    /// Records the source's data as `f` changes it.
    fn update(&self, f: impl FnOnce(&mut SourceData<ImageSourceType>)) {
        if let Some(record) = self.inner.base.base_impl().map_data(f) {
            self.inner.base.set_base_impl(record);
        }
    }

    pub fn set_coordinates(&self, coords: [LatLng; 4]) {
        self.update(|data| data.options.coordinates = coords);
        self.inner.base.observer().on_source_changed(self);
    }

    #[must_use]
    pub fn get_coordinates(&self) -> [LatLng; 4] {
        self.read(|data| data.options.coordinates).unwrap_or_default()
    }

    pub fn set_url(&self, url: &str) {
        self.update(|data| {
            data.options.url = Some(url.to_owned());
            data.load += 1;
        });
        self.inner.generation.set(self.inner.generation.get() + 1);
        // Signal that the source description needs a reload
        if self.inner.base.loaded() || self.inner.req.borrow().is_some() {
            self.inner.base.set_loaded(false);
            *self.inner.req.borrow_mut() = None;
            self.inner.base.observer().on_source_description_changed(self);
        }
    }

    pub fn set_image(&self, image: PremultipliedImage) {
        self.inner.generation.set(self.inner.generation.get() + 1);
        *self.inner.req.borrow_mut() = None;
        self.inner.base.set_loaded(true);
        self.update(|data| {
            data.options.url = None;
            data.description = Some(Arc::new(image));
            data.load += 1;
        });
        self.inner.base.observer().on_source_changed(self);
    }

    #[must_use]
    pub fn get_url(&self) -> Option<String> {
        self.read(|data| data.options.url.clone()).flatten()
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
        let weak = Weak::clone(weak);
        let generation = this.inner.generation.get();
        decoder::decode(&data, move |result| {
            let Some(inner) = weak.upgrade().filter(|inner| inner.generation.get() == generation) else {
                return;
            };
            let this = ImageSource { inner };
            let observer = this.inner.base.observer();
            match result {
                Ok(image) => {
                    this.update(|data| data.description = Some(Arc::new(image)));
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

    fn load_description(&self, file_source: &dyn FileSource, run_loop: &mln_actor::RunLoop) {
        let Some(url) = self.get_url() else {
            self.inner.base.set_loaded(true);
            return;
        };

        if self.inner.req.borrow().is_some() || self.inner.base.loaded() {
            return;
        }

        let weak = Rc::downgrade(&self.inner);
        let req = file_source.request(
            &run_loop.scheduler(),
            &Resource::image(&url),
            Box::new(move |res: Response| on_image_response(&weak, &res)),
        );
        *self.inner.req.borrow_mut() = Some(req);
    }
}
