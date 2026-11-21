//! The `symbol` layer type: a layer of a geometry source that draws its
//! features' labels and icons, which its layers place where they collide
//! with none placed before them.
//!
//! A tile lays a group of symbol layers out (`layout`) into a bucket of
//! symbol instances (`bucket`): each feature's anchors along its geometry
//! (`anchor`), its shaped text and icon as quads (`quads`) and their
//! collision geometry (`collision_feature`). The type's placement pass
//! ([`placement::SymbolPlacement`]) places the frame's symbols on the
//! engine's schedule, gives them ids that hold across tiles of different
//! zooms, and fades them, and the layer draws them where placement put them
//! (`projection`, `render_layer`).

mod anchor;
pub(crate) mod bucket;
mod collision_feature;
pub(crate) mod format_section_override;
pub(crate) mod instance;
pub(crate) mod layout;
pub mod placement;
mod pool;
pub mod programs;
mod projection;
#[rustfmt::skip]
mod properties;
mod quads;
pub(crate) mod render_layer;
pub mod ubo;
crate::shaders::language_modules!();

use mln_style::layer::FontStack;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
use mln_style::property_value::{PropertyValue, StyleValue as _};
use mln_util::constants::{LAST_RESORT_ALPHABETIC_FONT, LAST_RESORT_PAN_UNICODE_FONT};
use mln_util::logging::{Event, Log};
use mln_vec_map::VecSet;
pub use properties::{SymbolLayoutProperties, SymbolPaintProperties};

use self::format_section_override::SymbolLayerPaintPropertyOverrides;

/// The `symbol` layer type.
pub struct Symbol;

impl StyleLayerType for Symbol {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "symbol", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = SymbolLayoutProperties::Unevaluated;
    type PaintProperties = SymbolPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }

    fn has_section_overrides(style: &LayerStyle<Self>) -> bool {
        SymbolLayerPaintPropertyOverrides::has_overrides(&style.layout.text_field)
    }

    fn content_elevation(style: &LayerStyle<Self>) -> Option<&PropertyValue<f32>> {
        Some(&style.layout.symbol_height_offset)
    }

    fn populate_font_stack(style: &LayerStyle<Self>, font_stack: &mut VecSet<FontStack>) {
        if style.layout.text_field.is_undefined() {
            return;
        }

        match &style.layout.text_font {
            PropertyValue::Undefined => {
                font_stack
                    .insert(vec![LAST_RESORT_ALPHABETIC_FONT.to_owned(), LAST_RESORT_PAN_UNICODE_FONT.to_owned()]);
            }
            PropertyValue::Constant(constant) => {
                font_stack.insert(constant.clone());
            }
            PropertyValue::Expression(function) => {
                for value in function.possible_outputs() {
                    if let Some(value) = value {
                        font_stack.insert(value);
                    } else {
                        Log::warning(
                            Event::ParseStyle,
                            format!(
                                "Layer '{}' has an invalid value for text-font and will not render text. Output \
                                 values must be contained as literals within the expression.",
                                style.id
                            ),
                        );
                        break;
                    }
                }
            }
        }
    }
}
