// Ports src/mln/renderer/style_diff.hpp and src/mln/renderer/style_diff.cpp.
//
// C++ keeps added/removed/changed in `std::unordered_map`s; iteration order
// of those maps is unspecified there. The port uses `VecMap` (ordered by
// id); where the renderer iterates them (creating/removing render layers),
// the order has no observable effect.

use std::sync::Arc;

use mln_style::collection::ImmutableVector;
use mln_style::{image, layer, source};
use mln_util::longest_common_subsequence::longest_common_subsequence;
use mln_vec_map::VecMap;

#[derive(Clone, Debug)]
pub struct StyleChange<T: ?Sized> {
    pub before: Arc<T>,
    pub after: Arc<T>,
}

#[derive(Clone, Debug)]
pub struct StyleDifference<T: ?Sized> {
    pub added: VecMap<String, Arc<T>>,
    pub removed: VecMap<String, Arc<T>>,
    pub changed: VecMap<String, StyleChange<T>>,
}

impl<T: ?Sized> Default for StyleDifference<T> {
    fn default() -> Self {
        StyleDifference { added: VecMap::new(), removed: VecMap::new(), changed: VecMap::new() }
    }
}

fn diff<T: ?Sized>(
    a: &Arc<Vec<Arc<T>>>,
    b: &Arc<Vec<Arc<T>>>,
    id: impl Fn(&Arc<T>) -> String,
    eq: impl Fn(&Arc<T>, &Arc<T>) -> bool,
) -> StyleDifference<T> {
    let mut result = StyleDifference::default();

    if Arc::ptr_eq(a, b) {
        return result;
    }

    let lcs = longest_common_subsequence(a.as_slice(), b.as_slice(), &eq);

    let mut a_it = 0;
    let mut b_it = 0;
    let mut l_it = 0;

    // `emplace` keeps the first entry for a duplicate key, as C++ does.
    while a_it != a.len() || b_it != b.len() {
        if a_it != a.len() && (l_it == lcs.len() || !eq(&lcs[l_it], &a[a_it])) {
            result.removed.entry(id(&a[a_it])).or_insert_with(|| a[a_it].clone());
            a_it += 1;
        } else if b_it != b.len() && (l_it == lcs.len() || !eq(&lcs[l_it], &b[b_it])) {
            result.added.entry(id(&b[b_it])).or_insert_with(|| b[b_it].clone());
            b_it += 1;
        } else {
            if !Arc::ptr_eq(&a[a_it], &b[b_it]) {
                result
                    .changed
                    .entry(id(&b[b_it]))
                    .or_insert_with(|| StyleChange { before: a[a_it].clone(), after: b[b_it].clone() });
            }
            a_it += 1;
            b_it += 1;
            l_it += 1;
        }
    }

    result
}

pub type ImageDifference = StyleDifference<image::Impl>;
pub type SourceDifference = StyleDifference<dyn source::Impl>;
pub type LayerDifference = StyleDifference<dyn layer::Impl>;

#[must_use]
pub fn diff_images(a: &Arc<Vec<Arc<image::Impl>>>, b: &Arc<Vec<Arc<image::Impl>>>) -> ImageDifference {
    diff(a, b, |i| i.id.clone(), |lhs, rhs| lhs.id == rhs.id)
}

#[must_use]
pub fn diff_sources(a: &ImmutableVector<dyn source::Impl>, b: &ImmutableVector<dyn source::Impl>) -> SourceDifference {
    diff(
        a,
        b,
        |s| s.base().id.clone(),
        |lhs, rhs| lhs.base().id == rhs.base().id && lhs.base().type_ == rhs.base().type_,
    )
}

#[must_use]
pub fn diff_layers(a: &ImmutableVector<dyn layer::Impl>, b: &ImmutableVector<dyn layer::Impl>) -> LayerDifference {
    diff(a, b, |l| l.id.clone(), |lhs, rhs| lhs.id == rhs.id && std::ptr::eq(lhs.get_type_info(), rhs.get_type_info()))
}

#[must_use]
pub fn has_layout_difference(layer_diff: &LayerDifference, layer_id: &str) -> bool {
    if layer_diff.added.contains_key(layer_id) {
        return true;
    }
    match layer_diff.changed.get(layer_id) {
        None => false,
        Some(change) => change.before.has_layout_difference(&*change.after),
    }
}
