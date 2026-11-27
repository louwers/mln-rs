//! What a crate needs to write a source type: the engine half
//! ([`SourceType`], with its description's [`Load`], its [`Supply`] of
//! tiles from a [`DataOrigin`] or of an image, and the kinds of data it
//! supplies: [`GeometryTiles`], [`RasterTiles`] and [`DemTiles`]), the style
//! half ([`StyleSourceType`] and its [`SourceTypeInfo`]), and the
//! registration ([`RegistryBuilder::add_source_type`]).
//!
//! `mln_engine::source` describes the contract. What an origin fetches with
//! and the tiles it delivers are [`mln_engine::storage`]'s
//! and [`mln_engine::tile`]'s.

pub use mln_engine::source::*;
pub use mln_engine::{Registry, RegistryBuilder};
pub use mln_style::expression::functions::RegisterError;
pub use mln_style::source::{ErasedDescription, SourceCommon, SourceData, SourceTypeInfo, StyleSourceType};
