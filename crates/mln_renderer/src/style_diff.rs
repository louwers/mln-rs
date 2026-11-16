// The images and sources that differ between two of the style's id-sorted
// vectors, in `VecMap`s ordered by id. The layers' changes are
// `mln_style::diff::LayerChanges`.

use std::sync::Arc;

use mln_style::collection::ImmutableVector;
use mln_style::{image, source};
use mln_util::longest_common_subsequence::longest_common_subsequence;
use mln_vec_map::VecMap;

#[derive(Clone, Debug)]
pub(crate) struct StyleDifference<T: ?Sized> {
    pub(crate) added: VecMap<String, Arc<T>>,
    pub(crate) removed: VecMap<String, Arc<T>>,
    /// The new values of the items in both whose values differ.
    pub(crate) changed: VecMap<String, Arc<T>>,
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
            result.removed.entry(id(&a[a_it])).or_insert_with(|| Arc::clone(&a[a_it]));
            a_it += 1;
        } else if b_it != b.len() && (l_it == lcs.len() || !eq(&lcs[l_it], &b[b_it])) {
            result.added.entry(id(&b[b_it])).or_insert_with(|| Arc::clone(&b[b_it]));
            b_it += 1;
        } else {
            if !Arc::ptr_eq(&a[a_it], &b[b_it]) {
                result.changed.entry(id(&b[b_it])).or_insert_with(|| Arc::clone(&b[b_it]));
            }
            a_it += 1;
            b_it += 1;
            l_it += 1;
        }
    }

    result
}

pub(crate) type ImageDifference = StyleDifference<image::Impl>;
pub(crate) type SourceDifference = StyleDifference<dyn source::Impl>;

#[must_use]
pub(crate) fn diff_images(a: &Arc<Vec<Arc<image::Impl>>>, b: &Arc<Vec<Arc<image::Impl>>>) -> ImageDifference {
    diff(a, b, |i| i.id.clone(), |lhs, rhs| lhs.id == rhs.id)
}

#[must_use]
pub(crate) fn diff_sources(
    a: &ImmutableVector<dyn source::Impl>,
    b: &ImmutableVector<dyn source::Impl>,
) -> SourceDifference {
    diff(
        a,
        b,
        |s| s.base().id.clone(),
        |lhs, rhs| lhs.base().id == rhs.base().id && lhs.base().type_ == rhs.base().type_,
    )
}
