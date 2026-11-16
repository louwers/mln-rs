//! Changes to a style, as values: [`Edit`], which [`Style::apply`] makes,
//! and [`Parts`], which says which parts differ between two versions.

use std::sync::Arc;

use feature::Value;
use mln_style_types::VisibilityType;

use crate::expression::GlobalStateMap;
use crate::filter::Filter;
use crate::image;
use crate::layer::Layer;
use crate::light::Light;
use crate::projection::Projection;
use crate::sky::Sky;
use crate::source_type::Source;
use crate::style::Style;
use crate::transition_options::TransitionOptions;

/// A change to a style, which [`Style::apply`] makes.
#[expect(clippy::large_enum_variant, reason = "an edit is made and applied at once, not stored")]
pub enum Edit {
    /// Adds `layer` below the layer `before`, or on top when `before` is
    /// `None` or names no layer.
    AddLayer {
        layer: Layer,
        before: Option<String>,
    },
    RemoveLayer(String),
    /// Moves the layer `id` below the layer `before`, or on top when
    /// `before` is `None` or names no layer. A move below itself changes
    /// nothing.
    MoveLayer {
        id: String,
        before: Option<String>,
    },
    /// Replaces the layer with `layer`'s id.
    SetLayer(Layer),
    /// Sets a layer type's property, `<name>-transition`, `filter`,
    /// `minzoom`, `maxzoom`, `visibility`, `source` or `source-layer` from a
    /// style JSON value; null resets the ones that have a default.
    SetLayerProperty {
        layer: String,
        name: String,
        value: serde_json::Value,
    },
    SetLayerFilter {
        layer: String,
        filter: Filter,
    },
    SetLayerZoomRange {
        layer: String,
        min: f32,
        max: f32,
    },
    SetLayerVisibility {
        layer: String,
        visibility: VisibilityType,
    },
    SetLight(Light),
    /// Sets a property of the light from a style JSON value; null resets it.
    SetLightProperty {
        name: String,
        value: serde_json::Value,
    },
    /// Sets the sky; `None` removes it.
    SetSky(Option<Sky>),
    SetProjection(Projection),
    SetTransition(TransitionOptions),
    /// Sets a value that `global-state` expressions read. Null resets it to
    /// the default that the style's `state` gives.
    SetGlobalState {
        name: String,
        value: Value,
    },
    /// Replaces the defaults that the style's root `state` gives, and sets
    /// every global state value to its new default, as a style that loads
    /// with this `state` would.
    SetGlobalStateDefaults(GlobalStateMap),
    /// Adds an image, replacing the image with its id.
    AddImage(Arc<image::Impl>),
    /// Removes the image with this id; an id the style lacks is logged.
    RemoveImage(String),
    /// A sprite's images, each replacing the image with its id.
    AddImages(Vec<Arc<image::Impl>>),
    /// Records `source`, whose id is `id`, as the source whose handle the map holds,
    /// replacing that source's record in place or adding it last; `None`
    /// removes the record.
    SetSourceImpl {
        id: String,
        source: Option<Source>,
    },
}

/// The parts of a style that differ between two versions.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Parts(u16);

impl Parts {
    pub const LAYERS: Parts = Parts(1 << 0);
    pub const SOURCES: Parts = Parts(1 << 1);
    pub const IMAGES: Parts = Parts(1 << 2);
    pub const LIGHT: Parts = Parts(1 << 3);
    pub const SKY: Parts = Parts(1 << 4);
    pub const PROJECTION: Parts = Parts(1 << 5);
    pub const TRANSITION: Parts = Parts(1 << 6);
    pub const GLOBAL_STATE: Parts = Parts(1 << 7);
    /// The document's own members: its JSON, name, default camera, sprites,
    /// glyph URL and font faces, and the global state's defaults.
    pub const DOCUMENT: Parts = Parts(1 << 8);

    /// The parts whose `Arc`s differ between `old` and `new`, and the
    /// transition, compared by value. O(1): it walks nothing.
    #[must_use]
    pub fn between(old: &Style, new: &Style) -> Parts {
        old.parts_differing_from(new)
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether `self` has every part of `other`.
    #[must_use]
    pub fn contains(self, other: Parts) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) fn insert_if(&mut self, part: Parts, differs: bool) {
        if differs {
            self.0 |= part.0;
        }
    }
}

impl std::ops::BitOr for Parts {
    type Output = Parts;
    fn bitor(self, other: Parts) -> Parts {
        Parts(self.0 | other.0)
    }
}
