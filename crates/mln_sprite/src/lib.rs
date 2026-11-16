//! Sprites: the sprite loader, which requests the sprite JSON and image
//! through a `FileSource` and parses them on the thread pool, and the sprite
//! parser, whose `load_sprite` and `parse_sprite` take and return owned
//! values, as a job does.

mod sprite_loader;
mod sprite_loader_observer;
pub mod sprite_parser;

pub use sprite_loader::SpriteLoader;
pub use sprite_loader_observer::SpriteLoaderObserver;
pub use sprite_parser::{load_sprite, parse_sprite};

#[cfg(test)]
mod tests;
