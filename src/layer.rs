//! What a layer type's author writes with, from its home crates: the layer
//! contract ([`mln_render::layer`] and [`mln_render::draw`], whose docs
//! state it), placement for a type whose layers collide ([`placement`]),
//! the style half's names (`mln_style`), including the style
//! keys an extension declares outside its layer type's properties
//! ([`StyleKey`]), and the names of a program's declaration ([`program`],
//! with its shaders' [`source`]s).
//!
//! The smallest layer type, whose layers draw nothing, has both halves, a
//! registry that adds them and a style that loads a layer of it. The
//! example checks that the type registers and parses; its hooks run only on
//! a map, as those of the `mln_ngon` crate's `org.maplibre.ngon`, a whole
//! type that draws, do in its render tests:
//!
//! ```
//! use mln_rs::layer::{
//!     Draws, Layer, LayerTypeInfo, NewCx, NoProperties, NoTiles, PrepareCx, SourceKind, StyleLayerType,
//! };
//! use mln_rs::mln_render::RegistryBuilder;
//! use mln_rs::mln_style::Style;
//!
//! /// The style half: the type's name, its kind of source and its
//! /// properties.
//! struct Empty;
//!
//! impl StyleLayerType for Empty {
//!     const INFO: LayerTypeInfo = LayerTypeInfo { name: "org.example.empty", source: SourceKind::None, from_json: true };
//!     type LayoutProperties = NoProperties;
//!     type PaintProperties = NoProperties;
//!     type Data = ();
//! }
//!
//! /// The render half, one value per style layer of the type.
//! struct EmptyLayer;
//!
//! impl Layer for EmptyLayer {
//!     type Style = Empty;
//!     type Input = NoTiles;
//!     type Tile = ();
//!
//!     fn new(_cx: &mut NewCx<'_, Self>) -> Self {
//!         EmptyLayer
//!     }
//!
//!     fn prepare(&self, _cx: &PrepareCx<'_, Self>, _out: &mut Draws) {}
//! }
//!
//! let mut builder = RegistryBuilder::new();
//! builder.add_layer_type::<EmptyLayer>()?;
//! let registry = builder.build();
//!
//! let json = r#"{"version":8,"sources":{},"layers":[{"id":"nothing","type":"org.example.empty"}]}"#;
//! let style = Style::from_json(registry.style(), json, None)?;
//! assert!(style.get_layer("nothing").is_some());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub use mln_gfx::gfx_types::AttributeDataType;
pub use mln_gfx::gpu::{Caps, TextureFormat};
pub use mln_gfx::shaders::block::Bool32;
pub use mln_gfx::shaders::program::{
    self, Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Property, Sources, Stages,
};
pub use mln_gfx::shaders::source;
pub use mln_gfx::texture2d::SamplerState;
pub use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
pub use mln_gfx::{ColorMode, CullFaceMode, DrawMode, uniform_block};
pub use mln_map_types::{FrameProjection, MapMode, TileProjector, View};
pub use mln_render::draw::*;
pub use mln_render::layer::*;
pub use mln_render::{paint_property_binders, placement};
pub use mln_style::conversion::{self, Convertible};
pub use mln_style::evaluated::Evaluated;
pub use mln_style::expression::Functions;
pub use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType, ValueKind};
pub use mln_style::properties;
pub use mln_style::property_table::{PropertyInfo, PropertyTable, PropertyVisitor};
pub use mln_style::property_value::PropertyValue;
pub use mln_style::style_key::StyleKey;
pub use mln_style_types::image::ImageType;
pub use mln_style_types::{LayerBlendType, Rotation, TranslateAnchorType};
pub use mln_text::glyph::GlyphID;
pub use mln_tile_id::geometry_tile_data::{GeometryCoordinate, GeometryCoordinates};
pub use mln_util::error::Error;
pub use mln_util::geo::{LatLng, ScreenCoordinate};
pub use mln_util::size::Size;
pub use mln_util::{Color, intersection_tests};
pub use mln_wire::Wire;
