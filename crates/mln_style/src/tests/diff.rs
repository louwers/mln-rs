// `LayerChanges::between` and `StyleDiff::between`: which layers, sources
// and images changed between two versions, and how.

use std::sync::Arc;

use super::util::{new_layer, parse_json};
use crate::conversion::Convertible;
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
    let layers = list(&[&new_layer("plain", "a", "")]);
    let changes = LayerChanges::between(&layers, &layers);
    assert!(changes.is_empty() && !changes.order_changed);
}

// gtest: StyleDiff.Add
#[test]
fn add() {
    let (l1, l2) = (new_layer("plain", "id-1", "src-1"), new_layer("plain", "id-2", "src-2"));
    assert_eq!(changes(&[&l1], &[&l1, &l2]), [("id-2".to_owned(), LayerChange::Added)]);
    assert!(LayerChanges::between(&list(&[&l1]), &list(&[&l1, &l2])).order_changed);
}

// gtest: StyleDiff.Remove
#[test]
fn remove() {
    let (l1, l2) = (new_layer("plain", "id-1", "src-1"), new_layer("plain", "id-2", "src-2"));
    assert_eq!(changes(&[&l1, &l2], &[&l1]), [("id-2".to_owned(), LayerChange::Removed)]);
}

// Different identical instances show up as changed
// gtest: StyleDiff.Compare
#[test]
fn compare() {
    let old = [new_layer("dot", "id-1", "src-1"), new_layer("dot", "id-2", "src-2")];
    let new = [new_layer("dot", "id-1", "src-1"), new_layer("dot", "id-2", "src-2")];
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
    let (l1, l2) = (new_layer("plain", "id-1", "src-1"), new_layer("plain", "id-2", "src-2"));
    let changes = LayerChanges::between(&list(&[&l1, &l2]), &list(&[&l2, &l1]));
    assert!(changes.is_empty());
    assert!(changes.order_changed);
}

#[test]
fn a_moved_layer_that_changed_keeps_its_class() {
    let (a, b) = (new_layer("dot", "a", "s"), new_layer("dot", "b", "s"));
    let wider = changed(a.clone(), "dot-width", r#"["get", "width"]"#);
    let changes = LayerChanges::between(&list(&[&a, &b]), &list(&[&b, &wider]));
    assert_eq!(Some(LayerChange::Relaid), changes.get("a"));
    assert_eq!(None, changes.get("b"));
    assert!(changes.order_changed);
}

#[test]
fn a_layer_of_another_type_is_replaced() {
    let old = new_layer("plain", "a", "");
    let new = new_layer("dot", "a", "s");
    assert_eq!(changes(&[&old], &[&new]), [("a".to_owned(), LayerChange::Replaced)]);
}

#[test]
fn a_change_tiles_are_laid_out_with_is_relaid() {
    let dot = new_layer("dot", "l", "s");
    let cases = [
        ("dot-color", r##""#ff0000""##, LayerChange::Restyled),
        ("dot-width", r#"["get", "width"]"#, LayerChange::Relaid),
        ("dot-alignment", r#""map""#, LayerChange::Relaid),
        ("filter", r#"["==", "class", "road"]"#, LayerChange::Relaid),
        ("visibility", r#""none""#, LayerChange::Relaid),
    ];
    for (name, json, expected) in cases {
        let edited = changed(dot.clone(), name, json);
        assert_eq!(changes(&[&dot], &[&edited]), [("l".to_owned(), expected)], "{name}");
        assert_eq!(expected.relayouts(), expected == LayerChange::Relaid, "{name}");
    }
}

#[test]
fn a_change_of_data_alone_keeps_the_layer() {
    let plain = new_layer("plain", "p", "");
    let mut edited = plain.clone();
    crate::layer_type::LayerStyle::<crate::tests::fixture_layer::Plain>::edit_data(edited.style_mut(), |data| *data = 7)
        .unwrap();
    let data_changes = LayerChanges::between(&list(&[&plain]), &list(&[&edited]));
    assert_eq!(Some(LayerChange::Data), data_changes.get("p"));
    assert!(!data_changes.order_changed);
    assert!(!LayerChange::Data.relayouts() && !LayerChange::Data.restyles());

    // With a property changed too, the layer is restyled.
    let restyled = changed(edited, "plain-opacity", "0.5");
    assert_eq!(changes(&[&plain], &[&restyled]), [("p".to_owned(), LayerChange::Restyled)]);
}

// `StyleDiff::between`: what changed between two versions of a style.

mod versions {
    use std::sync::Arc;

    use mln_util::image::PremultipliedImage;
    use mln_util::size::Size;

    use crate::diff::{LayerChange, StyleDiff, sources_by_id};
    use crate::tests::fixture_source::{Points, Shapes, source};
    use crate::{Edit, StyleRegistry, image};

    const STYLE: &str = r#"{"version":8,
        "sources":{"c":{"type":"points"},"a":{"type":"points"}},
        "layers":[{"id":"bg","type":"plain"},{"id":"dots","type":"dot","source":"a"}]}"#;

    fn style() -> crate::Style {
        crate::Style::from_json(&StyleRegistry::with_builtins(), STYLE, None).unwrap()
    }

    fn shapes(id: &str) -> crate::Source {
        source::<Shapes>(id)
    }

    fn image(id: &str) -> Arc<image::Impl> {
        let pixels = PremultipliedImage::new(Size::new(2, 2));
        Arc::new(image::Impl::new(id.to_owned(), pixels, 1.0, false, Vec::new(), Vec::new(), None, None, None).unwrap())
    }

    fn keys<V>(map: &mln_vec_map::VecMap<String, V>) -> Vec<&str> {
        map.keys().map(String::as_str).collect()
    }

    #[test]
    fn a_renderer_with_nothing_gets_everything_added() {
        let mut style = style();
        style.apply(Edit::AddImage(image("i"))).unwrap();
        let diff = StyleDiff::between(None, &style);
        let layers: Vec<_> = diff.layers.iter().map(|(id, change)| (id.to_string(), change)).collect();
        assert_eq!(layers, [("bg".to_owned(), LayerChange::Added), ("dots".to_owned(), LayerChange::Added)]);
        assert!(diff.layers.order_changed);
        assert_eq!(["a", "c"], *keys(&diff.sources.added));
        assert_eq!(["i"], *keys(&diff.images.added));
        assert!(diff.sources.removed.is_empty() && diff.images.removed.is_empty());
    }

    #[test]
    fn a_layer_keeps_its_data_in_a_style_that_replaces_its_own() {
        use crate::tests::fixture_layer::{Dot, Plain};
        use crate::tests::util::style_of;

        let mut old = style();
        old.edit_layer_data::<Plain>("bg", |data| {
            *data = 7;
            Ok(())
        })
        .unwrap();
        let error = old.edit_layer_data::<Dot>("bg", |_| Ok(())).unwrap_err();
        assert_eq!(mln_util::ErrorKind::NotFound, error.kind);
        let mut new = style();
        new.keep_layer_data(&old);
        assert_eq!(7, style_of::<Plain>(new.get_layer("bg").unwrap()).data);
        // The same properties and the kept data: only the data may differ.
        assert_eq!(Some(LayerChange::Data), StyleDiff::between(Some(&old), &new).layers.get("bg"));
    }

    #[test]
    fn the_same_version_has_no_changes() {
        let style = style();
        let diff = StyleDiff::between(Some(&style), &style.clone());
        assert!(diff.layers.is_empty() && diff.sources.is_empty() && diff.images.is_empty());
    }

    #[test]
    fn the_source_pass_runs_in_id_order() {
        let mut style = style();
        style.apply(Edit::AddSource(shapes("b"))).unwrap();
        let ids: Vec<_> = style.get_sources().iter().map(|source| source.id().to_owned()).collect();
        assert_eq!(["c", "a", "b"], *ids);
        let sorted: Vec<_> = sources_by_id(style.get_sources()).iter().map(|source| source.id().to_owned()).collect();
        assert_eq!(["a", "b", "c"], *sorted);
    }

    #[test]
    fn sources_differ_by_id_record_and_type() {
        let old = style();
        let mut new = old.clone();
        for source in [shapes("c"), source::<Points>("a"), shapes("d")] {
            let _ = new.apply(Edit::RemoveSource(source.id().to_owned()));
            new.apply(Edit::AddSource(source)).unwrap();
        }
        let diff = StyleDiff::between(Some(&old), &new);
        assert_eq!(["c", "d"], *keys(&diff.sources.added));
        assert_eq!(["c"], *keys(&diff.sources.removed));
        assert_eq!(["a"], *keys(&diff.sources.changed));
    }

    #[test]
    fn images_differ_by_id_and_value() {
        let mut old = style();
        old.apply(Edit::AddImage(image("kept"))).unwrap();
        old.apply(Edit::AddImage(image("changed"))).unwrap();
        old.apply(Edit::AddImage(image("removed"))).unwrap();
        let mut new = old.clone();
        new.apply(Edit::AddImage(image("changed"))).unwrap();
        new.apply(Edit::RemoveImage("removed".to_owned())).unwrap();
        new.apply(Edit::AddImage(image("added"))).unwrap();
        let diff = StyleDiff::between(Some(&old), &new);
        assert_eq!(["added"], *keys(&diff.images.added));
        assert_eq!(["removed"], *keys(&diff.images.removed));
        assert_eq!(["changed"], *keys(&diff.images.changed));
    }

    #[test]
    fn each_layer_edit_names_its_layer_and_class() {
        let base = style();
        let cases = [
            ("dot-color", serde_json::json!("#ff0000"), LayerChange::Restyled),
            ("dot-width", serde_json::json!(["get", "radius"]), LayerChange::Relaid),
            ("filter", serde_json::json!(["==", "class", "road"]), LayerChange::Relaid),
            ("visibility", serde_json::json!("none"), LayerChange::Relaid),
        ];
        for (name, value, expected) in cases {
            let mut edited = base.clone();
            let edit = Edit::SetLayerProperty { layer: "dots".to_owned(), name: name.to_owned(), value };
            edited.apply(edit).unwrap();
            let diff = StyleDiff::between(Some(&base), &edited);
            let layers: Vec<_> = diff.layers.iter().map(|(id, change)| (id.to_string(), change)).collect();
            assert_eq!(layers, [("dots".to_owned(), expected)], "{name}");
        }

        // A global state key that a visibility expression reads.
        let mut stateful = base.clone();
        let visibility = serde_json::json!(["case", ["to-boolean", ["global-state", "show"]], "visible", "none"]);
        let edit =
            Edit::SetLayerProperty { layer: "dots".to_owned(), name: "visibility".to_owned(), value: visibility };
        stateful.apply(edit).unwrap();
        let mut shown = stateful.clone();
        shown.apply(Edit::SetGlobalState { name: "show".to_owned(), value: feature::Value::Bool(true) }).unwrap();
        let diff = StyleDiff::between(Some(&stateful), &shown);
        let layers: Vec<_> = diff.layers.iter().map(|(id, change)| (id.to_string(), change)).collect();
        assert_eq!(layers, [("dots".to_owned(), LayerChange::Relaid)]);
    }
}
