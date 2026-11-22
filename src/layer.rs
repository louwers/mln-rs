//! What a layer type's author writes with, from its home crates: the layer
//! contract ([`mln_render::layer`] and [`mln_render::draw`], whose docs
//! state it), the style half's names (`mln_style`), and the names of a
//! program's declaration ([`program`], with its shaders' [`source`]s).

pub use mln_gfx::gfx_types::AttributeDataType;
pub use mln_gfx::shaders::program::{self, Attribute, Program, Projection, Property, Sources, Stages};
pub use mln_gfx::shaders::source;
pub use mln_gfx::{ColorMode, CullFaceMode, DrawMode, uniform_block};
pub use mln_render::draw::*;
pub use mln_render::layer::*;
pub use mln_render::paint_property_binders;
pub use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
pub use mln_style::properties;
pub use mln_style::property_table::{PropertyInfo, PropertyTable, PropertyVisitor};
pub use mln_util::Color;
pub use mln_util::error::Error;
