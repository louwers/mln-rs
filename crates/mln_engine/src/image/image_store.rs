// The style's images, on the render thread, owned by the orchestrator. Tiles
// don't register with it: a tile waiting for images asks it to `resolve`
// them again after it changes (`revision`). An image missing once the sprite
// loaded is asked for with `styleimagemissing`, and resolves as missing in
// the next update, once the app has had its frame to add it.

use std::sync::Arc;

use mln_style::collections::{VecMap, VecSet};
use mln_style::hash::{HashMap, HashSet};
use mln_style::image::Impl as ImageImpl;

use crate::log_target;
use crate::render::events::RendererEvent;

/// Default `ImageManager`'s cache size for images added via
/// onStyleImageMissing API. Average sprite size with 1.0 pixel ratio is
/// ~2kB, 8kB for pixel ratio of 2.0.
pub(crate) const DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE: usize = 100 * 8192;

/// What a layout uses an image as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, mln_wire::Wire)]
pub enum ImageType {
    Icon,
    Pattern,
}

/// Images by id.
///
/// A `VecMap` rather than a `HashMap`:
/// `DynamicTextureAtlas::upload_icons_and_patterns` reserves atlas space in
/// iteration order, and a `HashMap`'s order changes from process to process,
/// which would move icons in the atlas and change rendered pixels between runs.
pub(crate) type ImageMap = VecMap<String, Arc<ImageImpl>>;

/// The images that a layout uses, by id.
pub(crate) type ImageDependencies = HashMap<String, ImageType>;

/// One wait for images, across the resolves it takes.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ImageWait {
    /// The update in which the wait found images missing and asked for them.
    asked: Option<u64>,
}

/// What a tile's image dependencies resolved to. A requested image absent
/// from both is missing.
pub(crate) struct ResolvedImages {
    pub(crate) icons: ImageMap,
    pub(crate) patterns: ImageMap,
}

pub struct ImageStore {
    /// Whether the sprite loaded: before, a missing image may still come
    /// with it.
    loaded: bool,
    images: ImageMap,
    /// The keys of `images`.
    available_images: Arc<VecSet<String>>,
    /// The images asked for with `styleimagemissing`, which the app may add
    /// and the store asks it to remove once no tile uses them.
    requested: VecSet<String>,
    /// The bytes of the requested images the app added.
    requested_images_cache_size: usize,
    /// The update each image was last asked for in.
    asked: VecMap<String, u64>,
    /// The orchestrator's updates, counted.
    update: u64,
    /// Whether this update asked for an image.
    asked_this_update: bool,
    revision: u64,
    /// What the store reported since the last `take_events`: the images it
    /// asked the app for and the ones it asks the app to remove.
    events: Vec<RendererEvent>,
}

impl Default for ImageStore {
    fn default() -> Self {
        ImageStore {
            loaded: false,
            images: ImageMap::new(),
            available_images: Arc::default(),
            requested: VecSet::new(),
            requested_images_cache_size: 0,
            asked: VecMap::new(),
            update: 0,
            asked_this_update: false,
            revision: 0,
            events: Vec::new(),
        }
    }
}

impl ImageStore {
    /// What the store reported since the last call, oldest first.
    pub(crate) fn take_events(&mut self) -> Vec<RendererEvent> {
        std::mem::take(&mut self.events)
    }

    /// Changes whenever a resolve that waited could now give another answer.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// Starts an update: the images asked for in the last one now resolve as
    /// missing if the app didn't add them.
    pub(crate) fn begin_update(&mut self) {
        self.update += 1;
        if std::mem::take(&mut self.asked_this_update) {
            self.revision += 1;
        }
    }

    pub(crate) fn set_loaded(&mut self, loaded: bool) {
        if self.loaded != loaded {
            self.loaded = loaded;
            self.revision += 1;
        }
    }

    pub(crate) fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub(crate) fn dump_debug_logs(&self) {
        log::info!(target: log_target::GENERAL, "ImageManager::loaded: {}", if self.is_loaded() { "1" } else { "0" });
    }

    #[must_use]
    pub fn image(&self, id: &str) -> Option<&Arc<ImageImpl>> {
        self.images.get(id)
    }

    pub(crate) fn available_images(&self) -> &Arc<VecSet<String>> {
        &self.available_images
    }

    pub(crate) fn add_image(&mut self, image: Arc<ImageImpl>) {
        debug_assert!(!self.images.contains_key(&image.id));
        if self.requested.contains(&image.id) {
            self.requested_images_cache_size += image.image.bytes();
        }
        Arc::make_mut(&mut self.available_images).insert(image.id.clone());
        self.images.entry(image.id.clone()).or_insert(image);
        self.revision += 1;
    }

    /// Replaces an image; true when the tiles that use it need a relayout:
    /// its size or SDF flag changed. Otherwise the atlas patches its bins.
    pub(crate) fn update_image(&mut self, image: Arc<ImageImpl>) -> bool {
        let Some(old_image) = self.images.get(&image.id) else {
            debug_assert!(false);
            return false;
        };
        let size_changed = old_image.image.size != image.image.size;
        let sdf_changed = old_image.sdf != image.sdf;
        if size_changed && self.requested.contains(&image.id) {
            self.requested_images_cache_size =
                self.requested_images_cache_size - old_image.image.bytes() + image.image.bytes();
        }
        self.images.insert(image.id.clone(), image);
        self.revision += 1;
        size_changed || sdf_changed
    }

    pub(crate) fn remove_image(&mut self, id: &str) {
        let Some(image) = self.images.remove(id) else {
            debug_assert!(false);
            return;
        };
        if self.requested.remove(id) {
            self.requested_images_cache_size -= image.image.bytes();
        }
        Arc::make_mut(&mut self.available_images).remove(id);
        self.revision += 1;
    }

    /// The images of `wants`, once each is there or known missing. Until
    /// then `None`: before the sprite loads, a missing image may still come
    /// with it; after, the store asks for it and waits an update for the app
    /// to add it.
    pub(crate) fn resolve(&mut self, wants: &ImageDependencies, wait: &mut ImageWait) -> Option<ResolvedImages> {
        let mut missing: Vec<&String> = wants.keys().filter(|id| !self.images.contains_key(*id)).collect();
        if !missing.is_empty() {
            if !self.loaded {
                return None;
            }
            match wait.asked {
                // The app had its update to add them.
                Some(asked) if asked < self.update => {}
                Some(_) => return None,
                None => {
                    missing.sort_unstable();
                    for id in missing {
                        self.requested.insert(id.clone());
                        // Once per update, however many tiles miss it.
                        if self.asked.insert(id.clone(), self.update) != Some(self.update) {
                            self.events.push(RendererEvent::StyleImageMissing(id.clone()));
                        }
                    }
                    wait.asked = Some(self.update);
                    self.asked_this_update = true;
                    return None;
                }
            }
        }

        let mut resolved = ResolvedImages { icons: ImageMap::new(), patterns: ImageMap::new() };
        for (id, type_) in wants {
            if let Some(image) = self.images.get(id) {
                let images = match type_ {
                    ImageType::Icon => &mut resolved.icons,
                    ImageType::Pattern => &mut resolved.patterns,
                };
                images.insert(id.clone(), Arc::clone(image));
            }
        }
        Some(resolved)
    }

    /// Asks the app to remove the requested images that it added and that
    /// no tile uses: none of `used`.
    pub(crate) fn reduce_memory_use(&mut self, used: &HashSet<String>) {
        let unused: Vec<String> =
            self.requested.iter().filter(|id| self.images.contains_key(*id) && !used.contains(*id)).cloned().collect();
        if !unused.is_empty() {
            self.events.push(RendererEvent::UnusedStyleImages(unused));
        }
    }

    /// Whether the requested images the app added take more than
    /// `DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE` bytes, so that
    /// `reduce_memory_use` should run.
    pub(crate) fn exceeds_cache_size_limit(&self) -> bool {
        self.requested_images_cache_size > DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE
    }

    pub(crate) fn clear(&mut self) {
        self.images.clear();
        self.available_images = Arc::default();
        self.requested.clear();
        self.requested_images_cache_size = 0;
        self.asked.clear();
        self.loaded = false;
        self.revision += 1;
    }
}
