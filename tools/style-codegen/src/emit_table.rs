// The property table of a layer type, the light or the sky, and the
// `Properties` impl that maps its slots to fields.

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
        if self.property.name == "icon-image" || self.property.name == "text-field" {
            info.push_str(".tokens()");
        }
        info
    }
}

/// The entries of a layer type: paint properties, then layout properties,
/// without the mock `line-floor-width`.
pub(crate) fn layer_entries<'a>(paint: &'a [Property], layout: &'a [Property]) -> Vec<Entry<'a>> {
    let paint = paint.iter().filter(|p| p.name != "line-floor-width").map(|property| Entry {
        property,
        paint: true,
        field: format!("paint.{}", snake(&property.name)),
    });
    let layout = layout.iter().map(|property| Entry {
        property,
        paint: false,
        field: format!("layout.{}", snake(&property.name)),
    });
    paint.chain(layout).collect()
}

/// The `use` paths the table needs.
pub(crate) fn imports() -> Vec<String> {
    vec!["crate::property_table::{Properties, PropertyInfo, PropertyTable, PropertyVisitor}".into()]
}

/// Writes `PROPERTIES` and `impl Properties for {owner}`. `changed` is the
/// body of `property_changed`, if the owner reacts to a change.
pub(crate) fn emit(w: &mut Writer, owner: &str, entries: &[Entry<'_>], changed: Option<&[String]>) {
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

    w.line(&format!("impl Properties for {owner} {{"));
    w.line("    const TABLE: &'static PropertyTable = &PROPERTIES;");
    w.blank();
    w.line("    fn visit<V: PropertyVisitor<Self>>(slot: usize, visitor: V) -> Option<V::Output> {");
    w.line("        Some(match slot {");
    for (slot, entry) in entries.iter().enumerate() {
        let kind = if entry.paint { "paint" } else { "layout" };
        let field = &entry.field;
        let one_line = format!("            {slot} => visitor.{kind}(|s| &s.{field}, |s| &mut s.{field}),");
        if one_line.len() <= MAX_WIDTH {
            w.line(&one_line);
        } else {
            w.line(&format!("            {slot} => visitor.{kind}(|s| &s.{field}, |s| &mut s.{field}),"));
        }
    }
    w.line("            _ => return None,");
    w.line("        })");
    w.line("    }");
    if let Some(changed) = changed {
        w.blank();
        w.line("    fn property_changed(&mut self, slot: usize) {");
        for line in changed {
            w.line(&format!("        {line}"));
        }
        w.line("    }");
    }
    w.line("}");
}
