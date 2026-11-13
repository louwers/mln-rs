// The ImageManager lives on the render thread (the orchestrator owns it,
// tiles share it as `Rc`, C++ `std::shared_ptr`), so its state is in a
// `RefCell` instead of behind the C++ recursive mutex. Requestors (geometry
// tiles) are kept as `Weak` references keyed by their address, as the C++
// maps key them by pointer. Requestors are notified after the state borrow
// is released, so a requestor may call back into the manager.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_actor::scheduler;
use mln_style_types::image::{ImageDependencies, ImageMap, ImageType, Impl as ImageImpl};
#[cfg(test)]
use mln_util::constants::DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE;
use mln_util::logging::{Event, Log};
use mln_vec_map::{VecMap, VecSet};

use crate::gfx::dynamic_texture_atlas::ImageVersionMap;

pub(crate) type ImageRequestPair = (ImageDependencies, u64);

pub(crate) trait ImageManagerObserver {
    fn on_style_image_missing(&self, _id: &str, done: Box<dyn FnOnce()>) {
        done();
    }
    fn on_remove_unused_style_images(&self, _ids: &[String]) {}
}

struct NullObserver;
impl ImageManagerObserver for NullObserver {}

/// The data members of the C++ `ImageRequestor` base class.
#[derive(Default)]
pub(crate) struct ImageRequestorBase {
    /// Pending requests are image requests that are waiting to be
    /// dispatched to the client.
    pending_requests: RefCell<VecSet<String>>,
}

/// Receives the images it requested from the `ImageManager`. Implementors
/// call `ImageManager::remove_requestor_key` when they go away.
pub(crate) trait ImageRequestor {
    fn requestor_base(&self) -> &ImageRequestorBase;

    /// An image this requestor asked for while it was missing has been
    /// added, after the requestor's request was answered without it.
    fn on_requested_image_added(&self) {}

    fn on_images_available(
        &self,
        icons: ImageMap,
        patterns: ImageMap,
        version_map: ImageVersionMap,
        image_correlation_id: u64,
    );

    fn add_pending_request(&self, image_id: &str) {
        self.requestor_base().pending_requests.borrow_mut().insert(image_id.to_owned());
    }
    fn has_pending_request(&self, image_id: &str) -> bool {
        self.requestor_base().pending_requests.borrow().contains(image_id)
    }
    fn has_pending_requests(&self) -> bool {
        !self.requestor_base().pending_requests.borrow().is_empty()
    }
    fn remove_pending_request(&self, image_id: &str) {
        self.requestor_base().pending_requests.borrow_mut().remove(image_id);
    }
}

/// A requestor's identity (`ImageRequestor*`).
pub(crate) type RequestorKey = usize;

pub(crate) fn requestor_key<T: ?Sized>(requestor: &Rc<T>) -> RequestorKey {
    Rc::as_ptr(requestor).cast::<()>() as usize
}

type RequestorEntry = (Weak<dyn ImageRequestor>, ImageRequestPair);

struct State {
    loaded: bool,
    requestors: VecMap<RequestorKey, RequestorEntry>,
    missing_image_requestors: VecMap<RequestorKey, RequestorEntry>,
    requested_images: VecMap<String, VecMap<RequestorKey, Weak<dyn ImageRequestor>>>,
    /// Requestors that asked for an image added since, to tell in
    /// `notify_if_missing_image_added`.
    image_requestors_to_relayout: VecMap<RequestorKey, Weak<dyn ImageRequestor>>,
    requested_images_cache_size: usize,
    images: ImageMap,
    // Mirror of 'ImageMap images;' keys.
    available_images: Arc<VecSet<String>>,
    updated_image_versions: ImageVersionMap,
}

/// A notification to deliver once the state borrow is released.
type Notification = (Weak<dyn ImageRequestor>, ImageMap, ImageMap, ImageVersionMap, u64);

/// Tracks requests for icon images from tile workers and sends responses
/// when the requests are fulfilled.
pub(crate) struct ImageManager {
    self_weak: Weak<ImageManager>,
    state: RefCell<State>,
    observer: RefCell<Rc<dyn ImageManagerObserver>>,
}

impl ImageManager {
    #[must_use]
    pub(crate) fn create() -> Rc<ImageManager> {
        Rc::new_cyclic(|self_weak| ImageManager {
            self_weak: Weak::clone(self_weak),
            state: RefCell::new(State {
                loaded: false,
                requestors: VecMap::new(),
                missing_image_requestors: VecMap::new(),
                requested_images: VecMap::new(),
                image_requestors_to_relayout: VecMap::new(),
                requested_images_cache_size: 0,
                images: ImageMap::new(),
                available_images: Arc::new(VecSet::new()),
                updated_image_versions: ImageVersionMap::new(),
            }),
            observer: RefCell::new(Rc::new(NullObserver)),
        })
    }

    pub(crate) fn set_observer(&self, observer: Option<Rc<dyn ImageManagerObserver>>) {
        *self.observer.borrow_mut() = observer.unwrap_or_else(|| Rc::new(NullObserver));
    }

    pub(crate) fn set_loaded(&self, loaded: bool) {
        let mut notifications = Vec::new();
        let requestors = {
            let mut state = self.state.borrow_mut();
            if state.loaded == loaded {
                return;
            }

            state.loaded = loaded;

            if !state.loaded {
                return;
            }
            std::mem::take(&mut state.requestors)
        };
        for (key, (requestor, pair)) in requestors {
            self.check_missing_and_notify(key, &requestor, &pair, &mut notifications);
        }
        Self::deliver(notifications);
    }

    pub(crate) fn is_loaded(&self) -> bool {
        self.state.borrow().loaded
    }

    pub(crate) fn dump_debug_logs(&self) {
        Log::info(Event::General, format!("ImageManager::loaded: {}", if self.is_loaded() { "1" } else { "0" }));
    }

    pub(crate) fn get_image(&self, id: &str) -> Option<Arc<ImageImpl>> {
        self.get_shared_image(id)
    }

    pub(crate) fn get_shared_image(&self, id: &str) -> Option<Arc<ImageImpl>> {
        self.state.borrow().images.get(id).cloned()
    }

    pub(crate) fn add_image(&self, image: Arc<ImageImpl>) {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        debug_assert!(!state.images.contains_key(&image.id));

        // Increase cache size if requested image was provided.
        if let Some(requestors) = state.requested_images.get(&image.id) {
            state.requested_images_cache_size += image.image.bytes();
            for (key, requestor) in requestors {
                // Pending requests receive the image through
                // notify_if_missing_image_added.
                if !state.missing_image_requestors.contains_key(key) {
                    state.image_requestors_to_relayout.entry(*key).or_insert_with(|| Weak::clone(requestor));
                }
            }
        }

        Arc::make_mut(&mut state.available_images).insert(image.id.clone());
        state.images.entry(image.id.clone()).or_insert(image);
    }

    /// Replaces an image; true when tiles need a relayout.
    pub(crate) fn update_image(&self, image: Arc<ImageImpl>) -> bool {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;

        let Some(old_image) = state.images.get(&image.id) else {
            debug_assert!(false);
            return false;
        };

        let size_changed = old_image.image.size != image.image.size;
        // A changed SDF flag requires a relayout so buckets pick up the new
        // pattern type. Same-size updates still bump the version so existing
        // layouts patch their pixels; the atlas allocates per image
        // revision, so the relayout gets a freshly uploaded bin.
        let sdf_changed = old_image.sdf != image.sdf;

        if size_changed {
            // Update cache size if requested image size has changed.
            if state.requested_images.contains_key(&image.id) {
                let diff = image.image.bytes() as i64 - old_image.image.bytes() as i64;
                state.requested_images_cache_size = (state.requested_images_cache_size as i64 + diff) as usize;
            }
            state.updated_image_versions.remove(&image.id);
        } else {
            *state.updated_image_versions.entry(image.id.clone()).or_insert(0) += 1;
        }

        state.images.insert(image.id.clone(), image);

        size_changed || sdf_changed
    }

    pub(crate) fn remove_image(&self, id: &str) {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let Some(image) = state.images.get(id) else {
            debug_assert!(false);
            return;
        };

        // Reduce cache size for requested images.
        if state.requested_images.contains_key(&image.id) {
            state.requested_images_cache_size -= image.image.bytes();
            state.requested_images.remove(id);
        }

        state.images.remove(id);
        Arc::make_mut(&mut state.available_images).remove(id);
        state.updated_image_versions.remove(id);
    }

    pub(crate) fn get_images(&self, requestor: &Rc<dyn ImageRequestor>, pair: ImageRequestPair) {
        // remove previous requests from this tile
        let key = requestor_key(requestor);
        self.remove_requestor_key(key);

        let weak = Rc::downgrade(requestor);
        let mut notifications = Vec::new();

        // If all the icon dependencies are already present ((i.e. if they've
        // been addeded via runtime styling), then notify the requestor
        // immediately. Otherwise, if the sprite has not loaded, then wait for
        // it. When the sprite has loaded check if all icons are available. If
        // any are missing, call `onStyleImageMissing` to give the user a
        // chance to provide the icon. If they are not provided by the next
        // frame we'll assume they are permanently missing.
        if self.is_loaded() {
            self.check_missing_and_notify(key, &weak, &pair, &mut notifications);
        } else {
            let mut state = self.state.borrow_mut();
            let has_all_dependencies = pair.0.keys().all(|id| state.images.contains_key(id));

            if has_all_dependencies {
                notifications.push(Self::notification(&state, &weak, &pair));
            } else {
                state.requestors.entry(key).or_insert((weak, pair));
            }
        }
        Self::deliver(notifications);
    }

    /// Removes the requestor with this identity key.
    pub(crate) fn remove_requestor_key(&self, key: RequestorKey) {
        let mut state = self.state.borrow_mut();

        state.image_requestors_to_relayout.remove(&key);
        state.requestors.remove(&key);
        state.missing_image_requestors.remove(&key);
        for requested_image in state.requested_images.values_mut() {
            requested_image.remove(&key);
        }
    }

    pub(crate) fn notify_if_missing_image_added(&self) {
        // Relayout once all of this update's images are added. A relayout
        // through a synchronous worker can change the set, so take the
        // requestors one at a time.
        loop {
            let next = {
                let mut state = self.state.borrow_mut();
                let key = state.image_requestors_to_relayout.keys().next().copied();
                key.and_then(|key| state.image_requestors_to_relayout.remove(&key))
            };
            let Some(requestor) = next else {
                break;
            };
            if let Some(requestor) = requestor.upgrade() {
                requestor.on_requested_image_added();
            }
        }

        let mut notifications = Vec::new();
        {
            let mut state = self.state.borrow_mut();
            let keys: Vec<RequestorKey> = state.missing_image_requestors.keys().copied().collect();
            for key in keys {
                let (weak, pair) = state.missing_image_requestors.get(&key).cloned().expect("requestor");
                let has_pending = weak.upgrade().is_some_and(|r| r.has_pending_requests());
                if !has_pending {
                    notifications.push(Self::notification(&state, &weak, &pair));
                    state.missing_image_requestors.remove(&key);
                }
            }
        }
        Self::deliver(notifications);
    }

    pub(crate) fn reduce_memory_use(&self) {
        let unused_ids: Vec<String> = {
            let state = self.state.borrow();
            state
                .requested_images
                .iter()
                .filter(|(id, requestors)| requestors.is_empty() && state.images.contains_key(*id))
                .map(|(id, _)| id.clone())
                .collect()
        };

        if !unused_ids.is_empty() {
            let observer = self.observer.borrow().clone();
            observer.on_remove_unused_style_images(&unused_ids);
        }
    }

    #[cfg(test)]
    pub(crate) fn reduce_memory_use_if_cache_size_exceeds_limit(&self) {
        if self.state.borrow().requested_images_cache_size > DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE {
            self.reduce_memory_use();
        }
    }

    pub(crate) fn get_available_images(&self) -> Arc<VecSet<String>> {
        Arc::clone(&self.state.borrow().available_images)
    }

    /// The versions of the updated images, by image id. Read by
    /// `GeometryTileRenderData::prepare`.
    #[cfg(test)]
    pub(crate) fn updated_image_versions(&self) -> ImageVersionMap {
        self.state.borrow().updated_image_versions.clone()
    }

    pub(crate) fn clear(&self) {
        let mut state = self.state.borrow_mut();

        debug_assert!(state.requestors.is_empty());
        debug_assert!(state.missing_image_requestors.is_empty());

        state.images.clear();
        state.available_images = Arc::new(VecSet::new());
        state.updated_image_versions.clear();
        state.requested_images.clear();
        state.image_requestors_to_relayout.clear();
        state.loaded = false;
    }

    fn check_missing_and_notify(
        &self,
        key: RequestorKey,
        requestor: &Weak<dyn ImageRequestor>,
        pair: &ImageRequestPair,
        notifications: &mut Vec<Notification>,
    ) {
        let mut state = self.state.borrow_mut();
        let mut missing_dependencies: Vec<String> = Vec::new();
        for id in pair.0.keys() {
            if !state.images.contains_key(id) {
                missing_dependencies.push(id.clone());
            } else if let Some(requestors) = state.requested_images.get_mut(id) {
                // The missing-image path below never registers images that are
                // already present.
                requestors.entry(key).or_insert(Weak::clone(requestor));
            }
        }

        if missing_dependencies.is_empty() {
            notifications.push(Self::notification(&state, requestor, pair));
            return;
        }

        debug_assert!(!state.missing_image_requestors.contains_key(&key));
        state.missing_image_requestors.entry(key).or_insert((Weak::clone(requestor), pair.clone()));

        let Some(requestor_rc) = requestor.upgrade() else {
            return;
        };
        let mut missing_calls: Vec<String> = Vec::new();
        for missing_image in &missing_dependencies {
            if let Some(existing_requestors) = state.requested_images.get_mut(missing_image) {
                // Already asked client about this image.
                // existingRequestors is empty if all the previous requestors
                // are deleted.
                let first_pending = existing_requestors
                    .values()
                    .next()
                    .and_then(Weak::upgrade)
                    .is_some_and(|r| r.has_pending_request(missing_image));
                if first_pending {
                    // Still waiting for the client response for this image.
                    requestor_rc.add_pending_request(missing_image);
                    existing_requestors.entry(key).or_insert(Weak::clone(requestor));
                    continue;
                }
                // The request for this image has been already delivered to
                // the client, so we do not treat it as pending.
                existing_requestors.entry(key).or_insert(Weak::clone(requestor));
                // TODO: we could `continue;` here, but we need to call
                // `observer->onStyleImageMissing`, so that rendering is
                // re-launched from the handler at Map::Impl.
            } else {
                state.requested_images.entry(missing_image.clone()).or_default().insert(key, Weak::clone(requestor));
                requestor_rc.add_pending_request(missing_image);
            }
            missing_calls.push(missing_image.clone());
        }
        drop(state);

        let observer = self.observer.borrow().clone();
        for missing_image in missing_calls {
            let weak_self = Weak::clone(&self.self_weak);
            let image_id = missing_image.clone();
            let remove_pending_requests = move || {
                let Some(this) = weak_self.upgrade() else {
                    return;
                };
                let state = this.state.borrow();
                let Some(existing_request) = state.requested_images.get(&image_id) else {
                    return;
                };

                for req in existing_request.values() {
                    if let Some(req) = req.upgrade() {
                        req.remove_pending_request(&image_id);
                    }
                }
            };
            observer.on_style_image_missing(&missing_image, bind_once_local(remove_pending_requests));
        }
    }

    /// The maps that answer `requestor`'s `pair`, for `deliver` to send.
    fn notification(state: &State, requestor: &Weak<dyn ImageRequestor>, pair: &ImageRequestPair) -> Notification {
        let mut icon_map = ImageMap::new();
        let mut pattern_map = ImageMap::new();
        let mut version_map = ImageVersionMap::with_capacity(pair.0.len());

        for (id, type_) in &pair.0 {
            if let Some(image) = state.images.get(id) {
                if *type_ == ImageType::Pattern {
                    pattern_map.entry(id.clone()).or_insert_with(|| Arc::clone(image));
                } else {
                    icon_map.entry(id.clone()).or_insert_with(|| Arc::clone(image));
                }

                if let Some(version) = state.updated_image_versions.get(id) {
                    version_map.entry(id.clone()).or_insert(*version);
                }
            }
        }

        (Weak::clone(requestor), icon_map, pattern_map, version_map, pair.1)
    }

    fn deliver(notifications: Vec<Notification>) {
        for (requestor, icons, patterns, versions, correlation_id) in notifications {
            if let Some(requestor) = requestor.upgrade() {
                requestor.on_images_available(icons, patterns, versions, correlation_id);
            }
        }
    }
}

/// Wraps `f` in a callable that runs it on the current thread's scheduler
/// (the render thread's run loop).
fn bind_once_local(f: impl FnOnce() + 'static) -> Box<dyn FnOnce()> {
    type Slot = RefCell<Option<Box<dyn FnOnce()>>>;
    let slot: Rc<Slot> = Rc::new(RefCell::new(Some(Box::new(f))));
    let run = |slot: &Rc<Slot>| {
        let f = slot.borrow_mut().take();
        if let Some(f) = f {
            f();
        }
    };
    match scheduler::try_get_current() {
        Some(scheduler) => {
            let object = mln_actor::LocalObject::from_rc(slot);
            Box::new(move || {
                scheduler.schedule(Box::new(move || {
                    if let Some(slot) = object.get() {
                        run(&slot);
                    }
                }));
            })
        }
        None => Box::new(move || run(&slot)),
    }
}
