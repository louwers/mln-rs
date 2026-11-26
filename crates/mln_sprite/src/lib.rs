//! Sprites: the sprite parser, whose `load_sprite` and `parse_sprite` take
//! and return owned values, as a job does. The map's style loader fetches a
//! sprite's index and image and runs `load_sprite` on its thread pool.

pub mod sprite_parser;

pub use sprite_parser::{load_sprite, parse_sprite};

/// The `log` targets of this crate's records. Each names the event that
/// `mln_c` reports the record under (`mln_c::logging`).
mod log_target {
    pub(crate) const SPRITE: &str = "mln_sprite::sprite";
}

#[cfg(test)]
mod tests;
