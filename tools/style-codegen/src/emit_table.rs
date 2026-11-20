// The property table of a layer type, the light or the sky, and the
// function that maps its slots to fields: a layer type's `visit`, which its
// `StyleLayerType` impl forwards to, or the light's or sky's `Properties`
// impl.

use crate::spec::Property;
use crate::types::{is_data_driven, snake};
use crate::writer::{MAX_WIDTH, Writer};

/// One property of a table: its spec entry, whether it is a paint property,
/// and the path of its field from the owner (`paint.fill_color`).
pub(crate) struct Entry<'a> {
    pub(crate) property: &'a Property,
    pub(crate) paint: bool,
    pub(crate) field: String,
}

impl Entry<'_> {
    fn info(&self) -> String {
        let mut info =
            format!("PropertyInfo::{}(\"{}\")", if self.paint { "paint" } else { "layout" }, self.property.name);
        if is_data_driven(self.property) {
            info.push_str(".data_driven()");
        }
        if self.property.flag("tokens") {
            info.push_str(".tokens()");
        }
        if self.property.flag("overridable") {
            info.push_str(".overridable()");
        }
        if self.property.property_type() == Some("cross-faded-data-driven") {
            info.push_str(".cross_faded()");
        }
        info
    }
}

/// The entries of a layer type: paint properties, then layout properties.
pub(crate) fn layer_entries<'a>(paint: &'a [Property], layout: &'a [Property]) -> Vec<Entry<'a>> {
    let paint =
        paint.iter().map(|property| Entry { property, paint: true, field: format!("paint.{}", snake(&property.name)) });
    let layout = layout.iter().map(|property| Entry {
        property,
        paint: false,
        field: format!("layout.{}", snake(&property.name)),
    });
    paint.chain(layout).collect()
}

/// How the table's slots map to fields.
pub(crate) enum Visit {
    /// A free `visit` function over the layer type's style data.
    Function,
    /// `impl Properties` for the owner.
    Impl,
}

/// The `use` paths the table needs, where `style` is the style crate's path.
pub(crate) fn imports(style: &str, visit: &Visit) -> Vec<String> {
    match visit {
        Visit::Function => vec![format!("{style}::property_table::{{PropertyInfo, PropertyTable, PropertyVisitor}}")],
        Visit::Impl => {
            vec![format!("{style}::property_table::{{Properties, PropertyInfo, PropertyTable, PropertyVisitor}}")]
        }
    }
}

/// Writes `PROPERTIES`, and `visit` or `impl Properties for {owner}`.
pub(crate) fn emit(w: &mut Writer, owner: &str, entries: &[Entry<'_>], visit: &Visit) {
    w.line("/// The properties, by slot.");
    w.line("pub(crate) static PROPERTIES: PropertyTable = PropertyTable::new(");
    w.line("    &[");
    for entry in entries {
        w.line(&format!("        {},", entry.info()));
    }
    w.line("    ],");
    let mut by_name: Vec<usize> = (0..entries.len()).collect();
    by_name.sort_by_key(|&slot| entries[slot].property.name.as_str());
    let slots: Vec<String> = by_name.iter().map(ToString::to_string).collect();
    let one_line = format!("    &[{}],", slots.join(", "));
    if one_line.len() <= MAX_WIDTH {
        w.line(&one_line);
    } else {
        w.line("    &[");
        let mut current = String::from("       ");
        for slot in &slots {
            if current.len() + 1 + slot.len() + 1 > MAX_WIDTH {
                w.line(current.trim_end());
                current = String::from("       ");
            }
            current.push(' ');
            current.push_str(slot);
            current.push(',');
        }
        w.line(&current);
        w.line("    ],");
    }
    w.line(");");
    w.blank();

    // The function's body, indented for a free function; an impl's method
    // is indented once more.
    let indent = match visit {
        Visit::Function => {
            w.line("/// Calls `visitor` with the field of the property in `slot`.");
            w.line(&format!(
                "pub(crate) fn visit<V: PropertyVisitor<{owner}>>(slot: usize, visitor: V) -> Option<V::Output> {{"
            ));
            ""
        }
        Visit::Impl => {
            w.line(&format!("impl Properties for {owner} {{"));
            w.line("    const TABLE: &'static PropertyTable = &PROPERTIES;");
            w.blank();
            w.line("    fn visit<V: PropertyVisitor<Self>>(slot: usize, visitor: V) -> Option<V::Output> {");
            "    "
        }
    };
    w.line(&format!("{indent}    Some(match slot {{"));
    for (slot, entry) in entries.iter().enumerate() {
        let kind = if entry.paint { "paint" } else { "layout" };
        let field = &entry.field;
        w.line(&format!("{indent}        {slot} => visitor.{kind}(|s| &s.{field}, |s| &mut s.{field}),"));
    }
    w.line(&format!("{indent}        _ => return None,"));
    w.line(&format!("{indent}    }})"));
    w.line(&format!("{indent}}}"));
    if let Visit::Impl = visit {
        w.line("}");
    }
}
