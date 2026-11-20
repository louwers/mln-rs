//! `plain`, a layer type without a source for the tests of this crate, which
//! can't reach the built-in layer types outside it. Its paint properties are
//! a constant color and opacity and a cross-faded pattern.

use crate::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use crate::property_table::{PropertyInfo, PropertyTable, PropertyVisitor};

crate::properties!(
    /// The paint properties of a `plain` layer.
    #[expect(non_snake_case, reason = "named as the layer types' property sets are")]
    #[expect(unreachable_pub, dead_code, reason = "a property set has the API of the layer types' sets")]
    pub(crate) mod PlainPaintProperties: paint {
        plain_color: constant Color = Color::black(),
        plain_opacity: constant f32 = 1.0_f32,
        plain_pattern: cross_faded Image = Image::default(),
    }
    use crate::expression::Image, mln_util::Color;
);

static PROPERTIES: PropertyTable = PropertyTable::new(
    &[PropertyInfo::paint("plain-color"), PropertyInfo::paint("plain-opacity"), PropertyInfo::paint("plain-pattern")],
    &[0, 1, 2],
);

/// The `plain` layer type.
pub(crate) struct Plain;

impl StyleLayerType for Plain {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "plain",
        source: SourceKind::None,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = PlainPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        Some(match slot {
            0 => visitor.paint(|s| &s.paint.plain_color, |s| &mut s.paint.plain_color),
            1 => visitor.paint(|s| &s.paint.plain_opacity, |s| &mut s.paint.plain_opacity),
            2 => visitor.paint(|s| &s.paint.plain_pattern, |s| &mut s.paint.plain_pattern),
            _ => return None,
        })
    }
}
