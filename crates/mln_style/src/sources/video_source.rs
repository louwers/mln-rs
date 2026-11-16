//! A `video` source: a video drawn on four geographic corners, which raster
//! layers draw as they draw an image source's image.
//!
//! The source only describes the video. The map opens a player of it
//! (`mln_video`), which sends frames to the source's [`FrameSlot`], and the
//! renderer draws the newest one.

use std::rc::Rc;
use std::sync::Arc;

use mln_storage::FileSource;
use mln_style_types::SourceType;
use mln_util::LatLng;
use mln_video::FrameSlot;

use crate::layer_type::{LayerTypeInfo, SourceKind};
use crate::source::{Source, SourceBase, downcast_arc};
use crate::source_impl::{self, ImplBase};

#[derive(Clone, Debug)]
pub struct Impl {
    base: ImplBase,
    coords: [LatLng; 4],
    urls: Arc<[String]>,
    frames: Arc<FrameSlot>,
}

impl Impl {
    #[must_use]
    fn new(id: String, urls: Vec<String>, coords: [LatLng; 4]) -> Self {
        Impl { base: ImplBase::new(SourceType::Video, id), coords, urls: urls.into(), frames: FrameSlot::new() }
    }

    /// The corners of the video, clockwise from the top left.
    #[must_use]
    pub fn get_coordinates(&self) -> [LatLng; 4] {
        self.coords
    }

    /// The video's URLs, in the order a player tries them.
    #[must_use]
    pub fn get_urls(&self) -> &[String] {
        &self.urls
    }

    /// Where the source's player sends its frames. A source keeps its slot
    /// through changes of its corners.
    #[must_use]
    pub fn frames(&self) -> &Arc<FrameSlot> {
        &self.frames
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

/// A handle on a video source, as the other sources are (see source.rs).
#[derive(Clone)]
pub struct VideoSource {
    inner: Rc<SourceBase>,
}

impl std::fmt::Debug for VideoSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoSource").field("base", &self.inner).finish()
    }
}

impl VideoSource {
    /// A source of the video at the first of `urls` a player plays, at
    /// `coords`, its four corners clockwise from the top left.
    pub fn new(id: impl Into<String>, urls: Vec<String>, coords: [LatLng; 4]) -> Self {
        let impl_: Arc<dyn source_impl::Impl> = Arc::new(Impl::new(id.into(), urls, coords));
        VideoSource { inner: Rc::new(SourceBase::new(impl_)) }
    }

    #[must_use]
    pub fn impl_(&self) -> Arc<Impl> {
        downcast_arc::<Impl>(&self.inner.base_impl())
    }

    pub fn set_coordinates(&self, coords: [LatLng; 4]) {
        let new_impl: Arc<dyn source_impl::Impl> = Arc::new(Impl { coords, ..(*self.impl_()).clone() });
        self.inner.set_base_impl(new_impl);
        self.inner.observer().on_source_changed(self);
    }

    #[must_use]
    pub fn get_coordinates(&self) -> [LatLng; 4] {
        self.impl_().get_coordinates()
    }

    #[must_use]
    pub fn get_urls(&self) -> Vec<String> {
        self.impl_().get_urls().to_vec()
    }
}

impl Source for VideoSource {
    fn base(&self) -> &SourceBase {
        &self.inner
    }

    /// The source has no description to load: its player loads the video.
    fn load_description(&self, _file_source: &dyn FileSource, _run_loop: &mln_actor::RunLoop) {
        self.inner.set_loaded(true);
    }

    fn supports_layer_type(&self, info: LayerTypeInfo) -> bool {
        info.source == SourceKind::Raster
    }

    fn create_mutable(&self) -> Box<dyn source_impl::Impl> {
        Box::new((*self.impl_()).clone())
    }
}
