use mln_util::constants::{LAST_RESORT_ALPHABETIC_FONT, LAST_RESORT_PAN_UNICODE_FONT};
use mln_util::logging::{Event, Log};
use mln_vec_map::VecSet;

use crate::layer::{FontStack, LayerTypeInfo, SourceKind};
use crate::layer_type::StyleLayerType;
pub use crate::layers::symbol_layer_impl::Impl;
use crate::layers::symbol_layer_impl::SymbolLayerPaintPropertyOverrides;
use crate::layers::symbol_layer_properties::{self, SymbolLayoutProperties, SymbolPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};
use crate::property_value::PropertyValue;

/// The `symbol` layer type.
pub struct Symbol;

impl StyleLayerType for Symbol {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "symbol",
        source: SourceKind::Geometry,
        pass_3d: false,
        fading_tiles: true,
        cross_tile_index: true,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &symbol_layer_properties::PROPERTIES;

    type LayoutProperties = SymbolLayoutProperties::Unevaluated;
    type PaintProperties = SymbolPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        symbol_layer_properties::visit(slot, visitor)
    }

    fn has_section_overrides(style: &Impl) -> bool {
        SymbolLayerPaintPropertyOverrides::has_overrides(&style.layout.text_field)
    }

    fn content_elevation(style: &Impl) -> Option<&PropertyValue<f32>> {
        Some(&style.layout.symbol_height_offset)
    }

    fn populate_font_stack(style: &Impl, font_stack: &mut VecSet<FontStack>) {
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
