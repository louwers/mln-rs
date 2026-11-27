//! Sprites: the sprite parser, whose `load_sprite` and `parse_sprite` take
//! and return owned values, as a job does. The map's style loader fetches a
//! sprite's index and image and runs `load_sprite` on its thread pool.

pub(crate) mod sprite_parser;

pub use sprite_parser::{load_sprite, parse_sprite};

#[cfg(test)]
mod tests;
