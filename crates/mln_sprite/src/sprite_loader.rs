// Ports src/mln/sprite/sprite_loader.hpp and src/mln/sprite/sprite_loader.cpp.
//
// The request callbacks and the parse reply reach the loader through a
// `Weak` (C++ captures `this`; the reply checks a `WeakPtrFactory`), so
// they do nothing once the loader is gone. C++ holds `dataMapMutex` while
// it calls the observer; here the data map's borrow ends first, so an
// observer may call back into the loader.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_actor::TaggedScheduler;
use mln_actor::scheduler::Respond;
use mln_platform_default_image::decoder;
use mln_storage::{AsyncRequest, Data, FileSource, Resource, Response};
use mln_style_types::image::Impl;
use mln_style_types::sprite::Sprite;
use mln_util::exception::{ExceptionPtr, RuntimeError};
use mln_util::immutable::Immutable;

use crate::sprite_loader_observer::{NullSpriteLoaderObserver, SpriteLoaderObserver};
use crate::sprite_parser::parse_sprite_images;

/// `SpriteLoader::Data`.
#[derive(Default)]
struct SpriteData {
    image: Option<Data>,
    json: Option<Data>,
    json_request: Option<Box<dyn AsyncRequest>>,
    sprite_request: Option<Box<dyn AsyncRequest>>,
}

struct Inner {
    data_map: RefCell<BTreeMap<String, SpriteData>>,
    observer: RefCell<Rc<dyn SpriteLoaderObserver>>,
    thread_pool: TaggedScheduler,
}

/// `SpriteLoader`.
pub struct SpriteLoader {
    pixel_ratio: f32,
    inner: Rc<Inner>,
}

/// The result `parseSprite` hands back from the thread pool.
struct ParseResult {
    images: Vec<Immutable<Impl>>,
    error: Option<ExceptionPtr>,
}

/// Which response a request callback handles.
#[derive(Clone, Copy)]
enum Part {
    Json,
    Image,
}

impl Inner {
    fn observer(&self) -> Rc<dyn SpriteLoaderObserver> {
        self.observer.borrow().clone()
    }

    /// The body of the two request callbacks in `load`.
    fn on_response(self: &Rc<Self>, sprite: &Sprite, part: Part, res: Response) {
        if let Some(error) = &res.error {
            self.observer().on_sprite_error(Some(sprite.clone()), RuntimeError::ptr(error.message.clone()));
            return;
        } else if res.not_modified {
            return;
        }
        {
            let mut data_map = self.data_map.borrow_mut();
            let Some(data) = data_map.get_mut(&sprite.id) else {
                return;
            };
            let value = if res.no_content {
                Some(Arc::new(Vec::new()))
            } else {
                // Only trigger a sprite loaded event we got new data.
                res.data
            };
            match part {
                Part::Json => data.json = value,
                Part::Image => data.image = value,
            }
        }
        self.emit_sprite_loaded_if_complete(sprite.clone());
    }

    /// `emitSpriteLoadedIfComplete(style::Sprite)`.
    fn emit_sprite_loaded_if_complete(self: &Rc<Self>, sprite: Sprite) {
        let (image, json) = {
            let data_map = self.data_map.borrow();
            let data = data_map.get(&sprite.id).expect("sprite data");
            match (&data.image, &data.json) {
                (Some(image), Some(json)) => (image.clone(), json.clone()),
                _ => return,
            }
        };

        let parse_sprite_copy = sprite.clone();
        let factory: Weak<Inner> = Rc::downgrade(self);
        // `parseSprite`, whose image decodes through the `decoder`, which
        // may answer after the closure returns (the browser's).
        self.thread_pool.schedule_and_reply_later(
            /* parseClosure */
            move |respond: Respond<ParseResult>| {
                decoder::decode(&image, move |raster| {
                    let parsed = raster
                        .map_err(RuntimeError::ptr)
                        .and_then(|raster| parse_sprite_images(&parse_sprite_copy.id, &raster, &json));
                    respond(match parsed {
                        Ok(images) => ParseResult { images, error: None },
                        Err(error) => ParseResult { images: Vec::new(), error: Some(error) },
                    });
                });
            },
            /* resultClosure */
            move |result: ParseResult| {
                if let Some(this) = factory.upgrade() {
                    let observer = this.observer();
                    match result.error {
                        Some(error) => observer.on_sprite_error(Some(sprite), error),
                        None => observer.on_sprite_loaded(Some(sprite), result.images),
                    }
                }
            },
        );
    }
}

impl SpriteLoader {
    /// `SpriteLoader(float pixelRatio, const TaggedScheduler& threadPool)`.
    pub fn new(pixel_ratio: f32, thread_pool: TaggedScheduler) -> Self {
        SpriteLoader {
            pixel_ratio,
            inner: Rc::new(Inner {
                data_map: RefCell::new(BTreeMap::new()),
                observer: RefCell::new(Rc::new(NullSpriteLoaderObserver)),
                thread_pool,
            }),
        }
    }

    /// `load(std::optional<style::Sprite>, FileSource&)`.
    pub fn load(&self, sprite: Option<Sprite>, file_source: &dyn FileSource) {
        let Some(sprite) = sprite else {
            // Treat a non-existent sprite as a successfully loaded empty sprite.
            self.inner.observer().on_sprite_loaded(None, Vec::new());
            return;
        };

        self.inner.observer().on_sprite_requested(&Some(sprite.clone()));

        let id = sprite.id.clone();
        let url = sprite.sprite_url.clone();

        // `dataMap[id] = std::make_unique<Data>()`: a previous load's
        // requests are cancelled (outside the borrow).
        let previous = self.inner.data_map.borrow_mut().insert(id.clone(), SpriteData::default());
        drop(previous);

        let make_callback = |part: Part| {
            let weak = Rc::downgrade(&self.inner);
            let sprite = sprite.clone();
            Box::new(move |res: Response| {
                if let Some(inner) = weak.upgrade() {
                    inner.on_response(&sprite, part, res);
                }
            })
        };

        let json_request =
            file_source.request(&Resource::sprite_json(&url, self.pixel_ratio), make_callback(Part::Json));
        if let Some(data) = self.inner.data_map.borrow_mut().get_mut(&id) {
            data.json_request = Some(json_request);
        }

        let sprite_request =
            file_source.request(&Resource::sprite_image(&url, self.pixel_ratio), make_callback(Part::Image));
        if let Some(data) = self.inner.data_map.borrow_mut().get_mut(&id) {
            data.sprite_request = Some(sprite_request);
        }
    }

    /// `setObserver(SpriteLoaderObserver*)`.
    pub fn set_observer(&self, observer: Rc<dyn SpriteLoaderObserver>) {
        *self.inner.observer.borrow_mut() = observer;
    }
}
