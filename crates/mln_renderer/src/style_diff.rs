// The images and sources that differ between two versions of the style, in
// `VecMap`s ordered by id. The layers' changes are
// `mln_style::diff::LayerChanges`.

use std::cmp::Ordering;
use std::sync::Arc;

use mln_style::{SourceList, image, source};
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

/// Merges `a` and `b`, both sorted by id with unique ids. An item whose id
/// is in both but that `same_kind` tells apart is removed and added.
fn diff<T: ?Sized>(
    a: &[Arc<T>],
    b: &[Arc<T>],
    id: impl Fn(&T) -> &str,
    same_kind: impl Fn(&T, &T) -> bool,
) -> StyleDifference<T> {
    let (mut removed, mut added, mut changed) = (Vec::new(), Vec::new(), Vec::new());
    let (mut a_it, mut b_it) = (a.iter().peekable(), b.iter().peekable());
    loop {
        let order = match (a_it.peek(), b_it.peek()) {
            (None, None) => break,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(old), Some(new)) => id(old).cmp(id(new)),
        };
        match order {
            Ordering::Less => removed.extend(a_it.next()),
            Ordering::Greater => added.extend(b_it.next()),
            Ordering::Equal => {
                let (Some(old), Some(new)) = (a_it.next(), b_it.next()) else { break };
                if !same_kind(old, new) {
                    removed.push(old);
                    added.push(new);
                } else if !Arc::ptr_eq(old, new) {
                    changed.push(new);
                }
            }
        }
    }
    // Each list is in id order, which the maps keep.
    let map = |items: Vec<&Arc<T>>| items.into_iter().map(|item| (id(item).to_owned(), Arc::clone(item))).collect();
    StyleDifference { added: map(added), removed: map(removed), changed: map(changed) }
}

pub(crate) type ImageDifference = StyleDifference<image::Impl>;
pub(crate) type SourceDifference = StyleDifference<dyn source::Impl>;

#[must_use]
pub(crate) fn diff_images(a: &Arc<Vec<Arc<image::Impl>>>, b: &Arc<Vec<Arc<image::Impl>>>) -> ImageDifference {
    if Arc::ptr_eq(a, b) {
        return ImageDifference::default();
    }
    diff(a, b, |image| &image.id, |_, _| true)
}

/// The sources of `list` in id order.
#[must_use]
pub(crate) fn sources_by_id(list: &SourceList) -> Vec<Arc<dyn source::Impl>> {
    let mut sources: Vec<_> = list.iter().cloned().collect();
    sources.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    sources
}

/// The sources that differ between `a` and `b`, both in id order. A source
/// whose type changed is removed and added.
#[must_use]
pub(crate) fn diff_sources(a: &[Arc<dyn source::Impl>], b: &[Arc<dyn source::Impl>]) -> SourceDifference {
    diff(a, b, |source| &source.id, |old, new| old.type_ == new.type_)
}

#[cfg(test)]
mod tests {
    use mln_style::source::Source;
    use mln_style::sources::geojson_source::{GeoJSONOptions, GeoJSONSource};
    use mln_style::sources::tile_source::TileSource;
    use mln_util::tileset::PartialTileset;

    use super::*;

    fn vector(id: &str) -> Arc<dyn source::Impl> {
        TileSource::vector(id, Some("mbtiles://tiles".to_owned()), PartialTileset::default()).base_impl()
    }

    fn ids(sources: &[Arc<dyn source::Impl>]) -> Vec<&str> {
        sources.iter().map(|source| source.id.as_str()).collect()
    }

    #[test]
    fn the_source_pass_runs_in_id_order() {
        let mut style = mln_style::Style::new(Arc::clone(crate::Registry::with_builtins().style()));
        for id in ["c", "b", "a"] {
            style.set_source_impl(id, Some(vector(id)));
        }
        assert_eq!(["a", "b", "c"], *ids(&sources_by_id(style.get_sources())));
    }

    #[test]
    fn sources_differ_by_id_record_and_type() {
        let (a, b, c) = (vector("a"), vector("b"), vector("c"));
        let geojson_b = GeoJSONSource::new("b", GeoJSONOptions::default_options()).base_impl();
        let old = [Arc::clone(&a), Arc::clone(&b), vector("d")];
        let new = [vector("a"), geojson_b, c];
        let diff = diff_sources(&old, &new);
        assert_eq!(["b", "c"], *diff.added.keys().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(["b", "d"], *diff.removed.keys().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(["a"], *diff.changed.keys().map(String::as_str).collect::<Vec<_>>());
        assert!(diff_sources(&old, &old).changed.is_empty());
    }
}
