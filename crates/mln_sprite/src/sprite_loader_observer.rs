use std::sync::Arc;

use mln_style_types::image::Impl;
use mln_style_types::sprite::Sprite;
use mln_util::error::Error;

/// Receives a sprite loader's results. Methods take `&self` (observers use
/// interior mutability); the default implementations do nothing.
pub trait SpriteLoaderObserver {
    fn on_sprite_loaded(&self, _sprite: Option<Sprite>, _images: Vec<Arc<Impl>>) {}

    fn on_sprite_error(&self, _sprite: Option<Sprite>, _error: Error) {}

    fn on_sprite_requested(&self, _sprite: &Option<Sprite>) {}
}

/// The C++ `nullObserver` (a default-constructed `SpriteLoaderObserver`).
pub struct NullSpriteLoaderObserver;

impl SpriteLoaderObserver for NullSpriteLoaderObserver {}
