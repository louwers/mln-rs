use std::sync::Arc;

use mln_style::layer::Impl;
use mln_style::layers::background_layer_impl::Impl as BackgroundLayerImpl;

use crate::style_diff::diff_layers;

fn layer(layer_id: &str, source_id: &str) -> Arc<dyn Impl> {
    Arc::new(BackgroundLayerImpl::new(layer_id.to_owned(), source_id.to_owned()))
}

type ImmutableLayer = Arc<dyn Impl>;

// gtest: StyleDiff.Empty
#[test]
fn empty() {
    let a: Vec<ImmutableLayer> = Vec::new();
    let b: Vec<ImmutableLayer> = Vec::new();
    let diff = diff_layers(&Arc::new(a), &Arc::new(b));
    assert!(diff.added.is_empty());
    assert!(diff.removed.is_empty());
    assert!(diff.changed.is_empty());
}

// gtest: StyleDiff.Add
#[test]
fn add() {
    let l1 = layer("id-1", "src-1");
    let l2 = layer("id-2", "src-2");

    let a: Vec<ImmutableLayer> = vec![Arc::clone(&l1)];
    let b: Vec<ImmutableLayer> = vec![Arc::clone(&l1), Arc::clone(&l2)];

    let diff = diff_layers(&Arc::new(a), &Arc::new(b));

    assert_eq!(1, diff.added.len());
    assert_eq!(0, diff.removed.len());
    assert_eq!(0, diff.changed.len());
}

// gtest: StyleDiff.Remove
#[test]
fn remove() {
    let l1 = layer("id-1", "src-1");
    let l2 = layer("id-2", "src-2");

    let a: Vec<ImmutableLayer> = vec![Arc::clone(&l1), Arc::clone(&l2)];
    let b: Vec<ImmutableLayer> = vec![Arc::clone(&l1)];

    let diff = diff_layers(&Arc::new(a), &Arc::new(b));

    assert_eq!(0, diff.added.len());
    assert_eq!(1, diff.removed.len());
    assert_eq!(0, diff.changed.len());
}

// Different identical instances show up as changed
// gtest: StyleDiff.Compare
#[test]
fn compare() {
    let a: Vec<ImmutableLayer> = vec![layer("id-1", "src-1"), layer("id-2", "src-2")];
    let b: Vec<ImmutableLayer> = vec![layer("id-1", "src-1"), layer("id-2", "src-2")];

    let diff = diff_layers(&Arc::new(a), &Arc::new(b));

    assert_eq!(0, diff.added.len());
    assert_eq!(0, diff.removed.len());
    assert_eq!(2, diff.changed.len());
}

// Order change shows up as add and remove
// gtest: StyleDiff.Order
#[test]
fn order() {
    let l1 = layer("id-1", "src-1");
    let l2 = layer("id-2", "src-2");

    let a: Vec<ImmutableLayer> = vec![Arc::clone(&l1), Arc::clone(&l2)];
    let b: Vec<ImmutableLayer> = vec![Arc::clone(&l2), Arc::clone(&l1)];

    let diff = diff_layers(&Arc::new(a), &Arc::new(b));

    assert_eq!(1, diff.added.len());
    assert_eq!(1, diff.removed.len());
    assert_eq!(0, diff.changed.len());
}
