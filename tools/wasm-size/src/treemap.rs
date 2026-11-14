//! The report as a page: a zoomable treemap of crate, module path and
//! function, colored by crate, or by change when there's a base.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{Sizes, closing, top_level_find};

const TEMPLATE: &str = include_str!("treemap.html");

/// The page, with the tree in it. It loads nothing else.
pub(crate) fn page(title: &str, sizes: &Sizes, base: Option<&Sizes>) -> String {
    let mut root = Node::default();
    for (name, item) in &sizes.items {
        root.insert(&path(name, item.krate.as_deref()), name, item.bytes, 0);
    }
    if let Some(base) = base {
        for (name, item) in &base.items {
            root.insert(&path(name, item.krate.as_deref()), name, 0, item.bytes);
        }
    }
    let data = json!({
        "title": title,
        "compare": base.is_some(),
        "tree": root.to_json(title, base.is_some()),
    });
    // `</script>` in a name mustn't end the page's script.
    let data = data.to_string().replace("</", "<\\/");
    TEMPLATE.replacen("\"__DATA__\"", &data, 1)
}

#[derive(Debug, Default)]
struct Node {
    size: u64,
    base: u64,
    /// The modules, types and function paths under it, and under a
    /// function path, its instances by full name.
    children: BTreeMap<Key, Node>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Group(String),
    Item(String),
}

impl Node {
    /// Adds an item under its path, its last group holding the item's
    /// instances: one, or one per set of generic arguments.
    fn insert(&mut self, groups: &[String], name: &str, size: u64, base: u64) {
        self.size += size;
        self.base += base;
        let child = match groups.split_first() {
            Some((group, rest)) => {
                self.children.entry(Key::Group(group.clone())).or_default().insert(rest, name, size, base);
                return;
            }
            None => self.children.entry(Key::Item(name.to_owned())).or_default(),
        };
        child.size += size;
        child.base += base;
    }

    /// `{n: name, s: size, b: base size, f: full name, c: children}`. A
    /// function path with one instance is that instance. A child no longer
    /// in the module is left out, but counts in `b`.
    fn to_json(&self, label: &str, compare: bool) -> Value {
        let mut node = json!({ "n": label, "s": self.size });
        if compare {
            node["b"] = self.base.into();
        }
        let live: Vec<_> = self.children.iter().filter(|(_, child)| child.size > 0).collect();
        if let [(Key::Item(name), _)] = live[..] {
            if name != label {
                node["f"] = name.as_str().into();
            }
            return node;
        }
        let children: Vec<Value> = live
            .into_iter()
            .map(|(key, child)| match key {
                Key::Group(group) => child.to_json(group, compare),
                // An instance, by its name from the function's on.
                Key::Item(name) => {
                    let mut leaf = child.to_json(name.rfind(label).map_or(name, |at| &name[at..]), compare);
                    leaf["f"] = name.as_str().into();
                    leaf
                }
            })
            .collect();
        if !children.is_empty() {
            node["c"] = children.into();
        }
        node
    }
}

/// Where an item goes in the tree: its crate, the path under it, and its own
/// name last. An item that isn't a named function is a group of its own.
fn path(name: &str, krate: Option<&str>) -> Vec<String> {
    let Some(krate) = krate else {
        return vec![name.to_owned()];
    };
    let mut segments = segments(name);
    if segments.first().is_some_and(|first| first == krate) {
        segments.remove(0);
    }
    if segments.is_empty() {
        segments.push(name.to_owned());
    }
    segments.insert(0, krate.to_owned());
    segments
}

/// A function's path without generic arguments, its self type's for a
/// method: `<a::B<T> as c::D>::f::<U>` is `a`, `B`, `f`.
fn segments(name: &str) -> Vec<String> {
    let mut path = String::new();
    let mut rest = name;
    if let Some(inner) = name.strip_prefix('<')
        && let Some(end) = closing(inner)
    {
        let qualified = &inner[..end];
        let self_type = top_level_find(qualified, " as ").map_or(qualified, |at| &qualified[..at]);
        path.push_str(self_type.trim_start_matches('&').trim_start_matches("mut ").trim_start_matches("dyn "));
        rest = &inner[end + 1..];
    }
    path.push_str(rest);
    let path = without_generics(&path);
    let mut segments = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b':' if depth == 0 && bytes.get(i + 1) == Some(&b':') => {
                segments.push(&path[start..i]);
                start = i + 2;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    segments.push(&path[start..]);
    segments.into_iter().filter(|segment| !segment.is_empty()).map(str::to_owned).collect()
}

fn without_generics(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0;
    let mut previous = '\0';
    for c in text.chars() {
        match c {
            '<' => depth += 1,
            '>' if previous == '-' => {}
            '>' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
        previous = c;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_drop_generics_and_follow_the_self_type() {
        assert_eq!(
            path("<mln_renderer::layers::Fill as mln_renderer::RenderLayer>::update", Some("mln_renderer")),
            ["mln_renderer", "layers", "Fill", "update"]
        );
        assert_eq!(
            path("core::ptr::drop_glue::<mln_map::map::Map>", Some("mln_map")),
            ["mln_map", "core", "ptr", "drop_glue"]
        );
        assert_eq!(
            path("<core::cell::OnceCell<T>>::try_init::<<F as Fn<()>>::Output, !>", Some("mln_tile")),
            ["mln_tile", "core", "cell", "OnceCell", "try_init"]
        );
        assert_eq!(path("<[usize]>::sort_by::<F>", Some("mln_text")), ["mln_text", "[usize]", "sort_by"]);
        assert_eq!(path("(data segments)", None), ["(data segments)"]);
    }

    #[test]
    fn the_tree_adds_up_and_groups_instances() {
        let mut root = Node::default();
        root.insert(&["a".into(), "m".into(), "f".into()], "a::m::f::<u8>", 10, 0);
        root.insert(&["a".into(), "m".into(), "f".into()], "a::m::f::<u16>", 5, 0);
        root.insert(&["a".into(), "h".into()], "a::h", 3, 0);
        root.insert(&["a".into(), "g".into()], "a::g", 0, 7);
        let tree = root.to_json("root", true);
        assert_eq!((&tree["s"], &tree["b"]), (&18.into(), &7.into()));
        let a = &tree["c"][0];
        assert_eq!(a["c"].as_array().unwrap().len(), 2, "a::g is gone");
        assert_eq!(a["c"][0], json!({ "n": "h", "s": 3, "b": 0, "f": "a::h" }));
        let f = &a["c"][1]["c"][0];
        assert_eq!((&f["n"], &f["s"]), (&"f".into(), &15.into()));
        assert_eq!(f["c"][1], json!({ "n": "f::<u8>", "s": 10, "b": 0, "f": "a::m::f::<u8>" }));
    }
}
