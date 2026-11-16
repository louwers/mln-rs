// `LayerChanges::between`: which layers changed between two layer lists, and
// how.

use std::sync::Arc;

use mln_conversion::Convertible;

use super::util::{new_layer, parse_json};
use crate::diff::{LayerChange, LayerChanges};
use crate::layer::Layer;
use crate::layer_list::LayerList;

fn list(layers: &[&Layer]) -> Arc<LayerList> {
    let mut list = LayerList::default();
    list.extend(layers.iter().map(|layer| (*layer).clone()));
    Arc::new(list)
}

fn changes(old: &[&Layer], new: &[&Layer]) -> Vec<(String, LayerChange)> {
    LayerChanges::between(&list(old), &list(new)).iter().map(|(id, change)| (id.to_string(), change)).collect()
}

fn changed(mut layer: Layer, name: &str, json: &str) -> Layer {
    let value = parse_json(json);
    assert!(layer.set_property(name, &Convertible::from(&value), None).is_none(), "{name}");
    layer
}

// gtest: StyleDiff.Empty
#[test]
fn empty() {
    let changes = LayerChanges::between(&list(&[]), &list(&[]));
    assert!(changes.is_empty());
    assert!(!changes.order_changed);
}

#[test]
fn the_same_list_has_no_changes() {
    let layers = list(&[&new_layer("background", "a", "")]);
    let changes = LayerChanges::between(&layers, &layers);
    assert!(changes.is_empty() && !changes.order_changed);
}

// gtest: StyleDiff.Add
#[test]
fn add() {
    let (l1, l2) = (new_layer("background", "id-1", "src-1"), new_layer("background", "id-2", "src-2"));
    assert_eq!(changes(&[&l1], &[&l1, &l2]), [("id-2".to_owned(), LayerChange::Added)]);
    assert!(LayerChanges::between(&list(&[&l1]), &list(&[&l1, &l2])).order_changed);
}

// gtest: StyleDiff.Remove
#[test]
fn remove() {
    let (l1, l2) = (new_layer("background", "id-1", "src-1"), new_layer("background", "id-2", "src-2"));
    assert_eq!(changes(&[&l1, &l2], &[&l1]), [("id-2".to_owned(), LayerChange::Removed)]);
}

// Different identical instances show up as changed
// gtest: StyleDiff.Compare
#[test]
fn compare() {
    let old = [new_layer("background", "id-1", "src-1"), new_layer("background", "id-2", "src-2")];
    let new = [new_layer("background", "id-1", "src-1"), new_layer("background", "id-2", "src-2")];
    let changes = LayerChanges::between(&list(&[&old[0], &old[1]]), &list(&[&new[0], &new[1]]));
    assert_eq!(Some(LayerChange::Restyled), changes.get("id-1"));
    assert_eq!(Some(LayerChange::Restyled), changes.get("id-2"));
    assert!(!changes.order_changed);
}

// An order change only changes the order: C++ and GL JS's diff show it as an
// addition and a removal.
// gtest: StyleDiff.Order
#[test]
fn order() {
    let (l1, l2) = (new_layer("background", "id-1", "src-1"), new_layer("background", "id-2", "src-2"));
    let changes = LayerChanges::between(&list(&[&l1, &l2]), &list(&[&l2, &l1]));
    assert!(changes.is_empty());
    assert!(changes.order_changed);
}

#[test]
fn a_moved_layer_that_changed_keeps_its_class() {
    let (a, b) = (new_layer("line", "a", "s"), new_layer("line", "b", "s"));
    let wider = changed(a.clone(), "line-width", r#"["get", "width"]"#);
    let changes = LayerChanges::between(&list(&[&a, &b]), &list(&[&b, &wider]));
    assert_eq!(Some(LayerChange::Relaid), changes.get("a"));
    assert_eq!(None, changes.get("b"));
    assert!(changes.order_changed);
}

#[test]
fn a_layer_of_another_type_is_replaced() {
    let old = new_layer("background", "a", "");
    let new = new_layer("line", "a", "s");
    assert_eq!(changes(&[&old], &[&new]), [("a".to_owned(), LayerChange::Replaced)]);
}

#[test]
fn a_change_tiles_are_laid_out_with_is_relaid() {
    let line = new_layer("line", "l", "s");
    let cases = [
        ("line-color", r##""#ff0000""##, LayerChange::Restyled),
        ("line-width", r#"["get", "width"]"#, LayerChange::Relaid),
        ("line-cap", r#""round""#, LayerChange::Relaid),
        ("filter", r#"["==", "class", "road"]"#, LayerChange::Relaid),
        ("visibility", r#""none""#, LayerChange::Relaid),
        ("line-pattern", r#""dots""#, LayerChange::Relaid),
    ];
    for (name, json, expected) in cases {
        let edited = changed(line.clone(), name, json);
        assert_eq!(changes(&[&line], &[&edited]), [("l".to_owned(), expected)], "{name}");
        assert_eq!(expected.relayouts(), expected == LayerChange::Relaid, "{name}");
    }
}
