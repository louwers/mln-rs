//! Sprites: the sprite loader, which requests the sprite JSON and image
//! through a `FileSource` and parses them on the thread pool, and the sprite
//! parser.

mod sprite_loader;
mod sprite_loader_observer;
pub mod sprite_parser;

pub use sprite_loader::SpriteLoader;
pub use sprite_loader_observer::SpriteLoaderObserver;
pub use sprite_parser::parse_sprite;

#[cfg(test)]
mod tests;
