// C++ `Source::Impl` is an abstract class with data members; the data is
// `ImplBase` and the virtual interface is the `Impl` trait (as for
// `Layer::Impl`). `dyn Impl` derefs to its `ImplBase`.

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use mln_style_types::SourceType;
use mln_util::constants;
use mln_util::logging::{Event, Log};

fn warn_if_overscale_factor_caps_prefetch_delta(overscale: Option<u8>, prefetch: Option<u8>) {
    let prefetch_delta = std::cmp::max(constants::DEFAULT_PREFETCH_ZOOM_DELTA, prefetch.unwrap_or(0));
    if let Some(overscale) = overscale
        && overscale < prefetch_delta
    {
        Log::warning(
            Event::Style,
            format!("Parent tile overscale factor will cap prefetch delta to {}", i32::from(overscale)),
        );
    }
}

/// The data members of `Source::Impl`.
#[derive(Clone, Debug)]
pub struct ImplBase {
    pub type_: SourceType,
    pub id: String,
    pub(crate) prefetch_zoom_delta: Option<u8>,
    pub(crate) max_overscale_factor: Option<u8>,
    pub(crate) minimum_tile_update_interval: Duration,
    pub(crate) volatile_flag: bool,
}

impl ImplBase {
    #[must_use]
    pub(crate) fn new(type_: SourceType, id: String) -> Self {
        ImplBase {
            type_,
            id,
            prefetch_zoom_delta: None,
            max_overscale_factor: None,
            minimum_tile_update_interval: Duration::ZERO,
            volatile_flag: false,
        }
    }

    pub(crate) fn set_prefetch_zoom_delta(&mut self, delta: Option<u8>) {
        self.prefetch_zoom_delta = delta;
        warn_if_overscale_factor_caps_prefetch_delta(self.max_overscale_factor, self.prefetch_zoom_delta);
    }

    #[must_use]
    pub fn get_prefetch_zoom_delta(&self) -> Option<u8> {
        self.prefetch_zoom_delta
    }

    pub(crate) fn set_minimum_tile_update_interval(&mut self, interval: Duration) {
        self.minimum_tile_update_interval = interval;
    }

    #[must_use]
    pub fn get_minimum_tile_update_interval(&self) -> Duration {
        self.minimum_tile_update_interval
    }

    pub(crate) fn set_max_overscale_factor_for_parent_tiles(&mut self, overscale_factor: Option<u8>) {
        self.max_overscale_factor = overscale_factor;
        warn_if_overscale_factor_caps_prefetch_delta(self.max_overscale_factor, self.prefetch_zoom_delta);
    }

    #[must_use]
    pub fn get_max_overscale_factor_for_parent_tiles(&self) -> Option<u8> {
        self.max_overscale_factor
    }

    #[must_use]
    pub fn is_volatile(&self) -> bool {
        self.volatile_flag
    }

    pub(crate) fn set_volatile(&mut self, set: bool) {
        self.volatile_flag = set;
    }
}

/// The virtual interface of `Source::Impl`.
pub trait Impl: Any + Send + Sync {
    fn base(&self) -> &ImplBase;
    fn base_mut(&mut self) -> &mut ImplBase;

    fn get_attribution(&self) -> Option<String>;

    fn is_update_synchronous(&self) -> bool {
        false
    }
}

impl dyn Impl {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: Impl>(&self) -> Option<&T> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }
}

impl std::ops::Deref for dyn Impl {
    type Target = ImplBase;
    fn deref(&self) -> &ImplBase {
        self.base()
    }
}

/// Returns a new handle to `impl_` as its concrete source impl type.
///
/// # Panics
///
/// If `impl_` is not a `T`.
pub fn downcast_arc<T: Impl>(impl_: &Arc<dyn Impl>) -> Arc<T> {
    let any: Arc<dyn Any + Send + Sync> = Arc::<dyn Impl>::clone(impl_);
    any.downcast::<T>().unwrap_or_else(|_| panic!("source impl type mismatch"))
}

// Debug is not a supertrait, for the reason at `impl Debug for dyn Layer`
// (layer.rs).
impl std::fmt::Debug for dyn Impl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let base = self.base();
        f.debug_struct("Source::Impl").field("id", &base.id).field("type", &base.type_).finish_non_exhaustive()
    }
}
