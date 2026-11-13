// Ports src/mln/style/source_impl.hpp and src/mln/style/source_impl.cpp.
//
// C++ `Source::Impl` is an abstract class with data members; the data is
// `ImplBase` and the virtual interface is the `Impl` trait (as for
// `Layer::Impl`). `dyn Impl` derefs to its `ImplBase`.

use std::any::Any;
use std::sync::Arc;

use mln_style_types::SourceType;
use mln_util::chrono::Duration;
use mln_util::constants;
use mln_util::immutable::Immutable;
use mln_util::logging::{Event, Log};

/// `WarnIfOverscaleFactorCapsPrefetchDelta`.
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
    pub prefetch_zoom_delta: Option<u8>,
    pub max_overscale_factor: Option<u8>,
    pub minimum_tile_update_interval: Duration,
    pub volatile_flag: bool,
}

impl ImplBase {
    /// `Impl(SourceType, std::string)`.
    #[must_use]
    pub fn new(type_: SourceType, id: String) -> Self {
        ImplBase {
            type_,
            id,
            prefetch_zoom_delta: None,
            max_overscale_factor: None,
            minimum_tile_update_interval: Duration::zero(),
            volatile_flag: false,
        }
    }

    pub fn set_prefetch_zoom_delta(&mut self, delta: Option<u8>) {
        self.prefetch_zoom_delta = delta;
        warn_if_overscale_factor_caps_prefetch_delta(self.max_overscale_factor, self.prefetch_zoom_delta);
    }

    #[must_use]
    pub fn get_prefetch_zoom_delta(&self) -> Option<u8> {
        self.prefetch_zoom_delta
    }

    pub fn set_minimum_tile_update_interval(&mut self, interval: Duration) {
        self.minimum_tile_update_interval = interval;
    }

    #[must_use]
    pub fn get_minimum_tile_update_interval(&self) -> Duration {
        self.minimum_tile_update_interval
    }

    pub fn set_max_overscale_factor_for_parent_tiles(&mut self, overscale_factor: Option<u8>) {
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

    pub fn set_volatile(&mut self, set: bool) {
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
        (self as &dyn Any).downcast_ref()
    }
}

impl std::ops::Deref for dyn Impl {
    type Target = ImplBase;
    fn deref(&self) -> &ImplBase {
        self.base()
    }
}

/// `staticImmutableCast<T>(impl)` for source impls. C++ uses `static_cast`; this
/// panics on a type mismatch.
pub fn static_immutable_cast<T: Impl>(impl_: &Immutable<dyn Impl>) -> Arc<T> {
    let any: Arc<dyn Any + Send + Sync> = impl_.clone();
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
