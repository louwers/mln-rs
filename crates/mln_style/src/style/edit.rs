//! Changes to a style, as values: [`Edit`], which
//! [`Style::apply`](crate::Style::apply) makes.

use std::sync::Arc;

use crate::error::Error;
use crate::expression::GlobalStateMap;
use crate::feature::Value;
use crate::filter::Filter;
use crate::image;
use crate::layer::Layer;
use crate::light::Light;
use crate::projection::Projection;
use crate::property::transition::TransitionOptions;
use crate::sky::Sky;
use crate::source::{ErasedDescription, Source, SourceCommon, SourceData, SourceInstance, StyleSourceType};
use crate::terrain::Terrain;
use crate::types::VisibilityType;

/// A change to a style, which [`Style::apply`](crate::Style::apply) makes.
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
    /// Sets the terrain; `None` removes it. Fails with `InvalidArgument` if
    /// the terrain's exaggeration is below 0 or not finite, or its source is
    /// in the style and isn't a `raster-dem` source.
    SetTerrain(Option<Terrain>),
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
    /// Adds `source` last. Fails when a layer that names its id can't draw
    /// a source of its kind.
    AddSource(Source),
    RemoveSource(String),
    /// Gives the source `id` the description a load of it found: `None`
    /// for one that found nothing. The load is the source's `instance` with
    /// its data's `load`; once the source moved on from that load, this
    /// changes nothing.
    SetSourceDescription {
        id: String,
        instance: SourceInstance,
        load: u64,
        description: Option<ErasedDescription>,
    },
    /// Sets the members every source has.
    SetSourceCommon {
        id: String,
        common: SourceCommon,
    },
    /// Changes the data of the source `id` as `update` says. A source type
    /// makes the updates its sources take, such as
    /// [`GeoJsonSourceType::set_url`](crate::sources::GeoJsonSourceType::set_url);
    /// [`Edit::update_source`] makes one from a change to a source's data.
    UpdateSource {
        id: String,
        update: SourceUpdate,
    },
}

/// What [`Edit::UpdateSource`] does to a source: it returns the source with
/// new data, made by [`Source::with_data`], or `None` if it changes nothing,
/// or fails, as for a source of a type it doesn't update.
pub type SourceUpdate = Box<dyn FnOnce(&Source) -> Result<Option<Source>, Error> + Send>;

impl Edit {
    /// The edit that changes the data of the source `id`, a `T` source:
    /// `change` changes a copy of the data and returns whether it changed
    /// anything. It fails with `InvalidArgument` for a source of another
    /// type, which isn't `kind`, as in `source "water" is not a GeoJSON
    /// source`.
    pub fn update_source<T: StyleSourceType>(
        id: impl Into<String>,
        kind: &'static str,
        change: impl FnOnce(&mut SourceData<T>) -> bool + Send + 'static,
    ) -> Edit {
        let update = move |source: &Source| {
            let Some(data) = SourceData::<T>::recover(source) else {
                return Err(Error::invalid_argument(format!("source \"{}\" is not {kind}", source.id())));
            };
            let mut data = data.clone();
            Ok(change(&mut data).then(|| source.with_data(data)))
        };
        Edit::UpdateSource { id: id.into(), update: Box::new(update) }
    }
}
