// Ports test/renderer/style_diff.test.cpp.

use std::sync::Arc;

use mln_renderer::style_diff::diff_layers;
use mln_style::layer::{Impl, ImplBase, LayerTypeInfo, layer_type_info};
use mln_util::immutable::Immutable;

#[derive(Debug)]
struct TestLayerImpl {
    base: ImplBase,
}

impl TestLayerImpl {
    fn create(layer_id: &str, source_id: &str) -> Immutable<dyn Impl> {
        Arc::new(TestLayerImpl { base: ImplBase::new(layer_id.to_owned(), source_id.to_owned()) })
    }
}

static TYPE_INFO: LayerTypeInfo = LayerTypeInfo {
    type_: "test",
    source: layer_type_info::Source::NotRequired,
    pass3d: layer_type_info::Pass3D::NotRequired,
    layout: layer_type_info::Layout::NotRequired,
    fading_tiles: layer_type_info::FadingTiles::NotRequired,
    cross_tile_index: layer_type_info::CrossTileIndex::NotRequired,
    tile_kind: layer_type_info::TileKind::NotRequired,
};

impl Impl for TestLayerImpl {
    fn base(&self) -> &ImplBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ImplBase {
        &mut self.base
    }

    fn has_layout_difference(&self, _other: &dyn Impl) -> bool {
        false
    }

    fn stringify_layout(&self, _writer: &mut String) {}

    fn get_type_info(&self) -> &'static LayerTypeInfo {
        &TYPE_INFO
    }
}

type ImmutableLayer = Immutable<dyn Impl>;

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
    let l1 = TestLayerImpl::create("id-1", "src-1");
    let l2 = TestLayerImpl::create("id-2", "src-2");

    let a: Vec<ImmutableLayer> = vec![l1.clone()];
    let b: Vec<ImmutableLayer> = vec![l1.clone(), l2.clone()];

    let diff = diff_layers(&Arc::new(a), &Arc::new(b));

    assert_eq!(1, diff.added.len());
    assert_eq!(0, diff.removed.len());
    assert_eq!(0, diff.changed.len());
}

// gtest: StyleDiff.Remove
#[test]
fn remove() {
    let l1 = TestLayerImpl::create("id-1", "src-1");
    let l2 = TestLayerImpl::create("id-2", "src-2");

    let a: Vec<ImmutableLayer> = vec![l1.clone(), l2.clone()];
    let b: Vec<ImmutableLayer> = vec![l1.clone()];

    let diff = diff_layers(&Arc::new(a), &Arc::new(b));

    assert_eq!(0, diff.added.len());
    assert_eq!(1, diff.removed.len());
    assert_eq!(0, diff.changed.len());
}

// Different identical instances show up as changed
// gtest: StyleDiff.Compare
#[test]
fn compare() {
    let a: Vec<ImmutableLayer> = vec![TestLayerImpl::create("id-1", "src-1"), TestLayerImpl::create("id-2", "src-2")];
    let b: Vec<ImmutableLayer> = vec![TestLayerImpl::create("id-1", "src-1"), TestLayerImpl::create("id-2", "src-2")];

    let diff = diff_layers(&Arc::new(a), &Arc::new(b));

    assert_eq!(0, diff.added.len());
    assert_eq!(0, diff.removed.len());
    assert_eq!(2, diff.changed.len());
}

// Order change shows up as add and remove
// gtest: StyleDiff.Order
#[test]
fn order() {
    let l1 = TestLayerImpl::create("id-1", "src-1");
    let l2 = TestLayerImpl::create("id-2", "src-2");

    let a: Vec<ImmutableLayer> = vec![l1.clone(), l2.clone()];
    let b: Vec<ImmutableLayer> = vec![l2.clone(), l1.clone()];

    let diff = diff_layers(&Arc::new(a), &Arc::new(b));

    assert_eq!(1, diff.added.len());
    assert_eq!(1, diff.removed.len());
    assert_eq!(0, diff.changed.len());
}
