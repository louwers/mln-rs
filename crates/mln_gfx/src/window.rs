//! The windows that window targets present to.

pub use raw_window_handle;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// A native window, by its handle and its display's: what a backend makes
/// the surface that a window target presents to with. The handles must stay
/// valid while a target uses them.
pub trait Window: HasWindowHandle + HasDisplayHandle + Send + Sync {}

impl<T: HasWindowHandle + HasDisplayHandle + Send + Sync + ?Sized> Window for T {}
