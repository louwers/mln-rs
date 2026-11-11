//! Ports `src/mln/sprite`: the sprite loader (requests the sprite JSON and
//! image through a FileSource, parses them on the thread pool) and the
//! sprite parser.

pub mod sprite_loader;
pub mod sprite_loader_observer;
pub mod sprite_parser;

pub use sprite_loader::SpriteLoader;
pub use sprite_loader_observer::SpriteLoaderObserver;
pub use sprite_parser::{create_style_image, parse_sprite, parse_sprite_images};

#[cfg(test)]
mod tests;
