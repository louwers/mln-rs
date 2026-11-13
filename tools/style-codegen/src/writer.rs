// Output helpers: a line writer and the `use` lines of the generated files.

use std::collections::BTreeMap;

use crate::spec::Property;
use crate::types::{evaluated_type, is_color_ramp};

/// The line width the templates wrap at (`split120Line`, `reduceConditions`).
pub const MAX_WIDTH: usize = 120;

/// `if a || b || ... {` at `indent`, wrapped after `||` past
/// [`MAX_WIDTH`], continuation lines indented four more spaces.
#[must_use]
pub fn if_condition(indent: &str, terms: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = format!("{indent}if {}", terms[0]);
    for term in &terms[1..] {
        if current.len() + 4 + term.len() + 2 > MAX_WIDTH {
            current.push_str(" ||");
            lines.push(current);
            current = format!("{indent}    {term}");
        } else {
            current.push_str(" || ");
            current.push_str(term);
        }
    }
    current.push_str(" {");
    lines.push(current);
    lines
}

pub struct Writer {
    out: String,
}

impl Writer {
    #[must_use]
    pub fn new() -> Self {
        Writer { out: String::new() }
    }

    pub fn line(&mut self, line: &str) {
        if line.is_empty() {
            self.out.push('\n');
        } else {
            self.out.push_str(line);
            self.out.push('\n');
        }
    }

    pub fn blank(&mut self) {
        self.out.push('\n');
    }

    /// `use` lines, merged per module and sorted.
    pub fn imports(&mut self, paths: Vec<String>) {
        let mut modules: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in paths {
            let (module, items) = if let Some(i) = path.find("::{") {
                let items = path[i + 3..path.len() - 1].split(',').map(|s| s.trim().to_owned()).collect();
                (path[..i].to_owned(), items)
            } else {
                let i = path.rfind("::").expect("a path with a module");
                (path[..i].to_owned(), vec![path[i + 2..].to_owned()])
            };
            modules.entry(module).or_default().extend(items);
        }
        for (module, mut items) in modules {
            items.sort_by(|a, b| {
                let key = |s: &str| (s != "self", s.to_owned());
                key(a).cmp(&key(b))
            });
            items.dedup();
            self.use_items("use", &module, &items);
        }
    }

    /// Writes `{keyword} {module}::{items};` on one line, or one item group
    /// per line, as rustfmt breaks it, when that is wider than [`MAX_WIDTH`].
    pub fn use_items(&mut self, keyword: &str, module: &str, items: &[String]) {
        let one_line = if items.len() == 1 {
            format!("{keyword} {module}::{};", items[0])
        } else {
            format!("{keyword} {module}::{{{}}};", items.join(", "))
        };
        if one_line.len() <= MAX_WIDTH {
            self.line(&one_line);
            return;
        }
        self.line(&format!("{keyword} {module}::{{"));
        let mut current = String::from("   ");
        for item in items {
            if current.len() + 1 + item.len() + 1 > MAX_WIDTH {
                self.line(current.trim_end());
                current = String::from("   ");
            }
            current.push(' ');
            current.push_str(item);
            current.push(',');
        }
        self.line(&current);
        self.line("};");
    }

    /// Drops the blank line(s) before a closing brace.
    pub fn finish_block(&mut self) {
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
    }

    #[must_use]
    pub fn finish(mut self) -> String {
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        self.out
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

/// `type.replace('-', '_')` in generate-style-code.mjs (the first hyphen
/// only; no layer type has two).
#[must_use]
pub fn layer_file_name(type_: &str) -> String {
    type_.replacen('-', "_", 1)
}

/// The `use` paths of the value types the properties mention.
pub fn imports_for<'a>(properties: impl Iterator<Item = &'a Property>, with_color_ramp: bool) -> Vec<String> {
    let mut names = Vec::new();
    let mut color_ramp = false;
    for p in properties {
        if is_color_ramp(p) {
            color_ramp = true;
            continue;
        }
        evaluated_type(p).named_types(&mut names);
    }
    let mut paths = Vec::new();
    for name in names {
        paths.push(type_path(&name));
    }
    if color_ramp && with_color_ramp {
        paths.push("crate::color_ramp_property_value::ColorRampPropertyValue".into());
    }
    paths
}

fn type_path(name: &str) -> String {
    match name {
        "Color" | "Padding" => format!("mln_util::{name}"),
        "Image" | "Formatted" => format!("crate::expression::{name}"),
        _ => format!("mln_style_types::{name}"),
    }
}
