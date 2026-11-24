//! The map handle's style commands: sources, images, layers, the light, the
//! global state and transitions, and the generic paths through which a
//! source type's own commands add, change and read its sources.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mln_style::image::Impl as StyleImage;
use mln_style::layer_type::StyleLayerType;
use mln_style::source_type::{SourceData, StyleSourceType};
use mln_style::{Edit, Source, SourceInstance, TransitionOptions};
use mln_util::error::Error;
use mln_util::hash::HashMap;

use crate::completion::{Completion, Resolver};
use crate::map_handle::{Applied, Map, MapEntry, MapId, apply_command, flush, missing_map, style_not_loaded};
use crate::runtime::{Background, Outbox, RuntimeCore, post};

/// A source of the map's style.
#[derive(Clone, Debug)]
pub struct SourceInfo {
    pub id: String,
    /// The source's [type](https://maplibre.org/maplibre-style-spec/sources/#type),
    /// such as `vector`, or the name of a type that only the API creates,
    /// such as `custom-geometry`.
    pub source_type: &'static str,
    pub is_volatile: bool,
    /// The source's attribution. A tile source with a URL has none until its
    /// TileJSON loads.
    pub attribution: Option<String>,
    /// The URL the source loads its data or description from.
    pub url: Option<String>,
    /// The source itself, from which its type's own settings read the rest,
    /// such as `mln_sources`' `TilesetSettings`.
    pub source: Source,
}

impl SourceInfo {
    /// What the API reports about `source`.
    #[must_use]
    pub fn of(source: &Source) -> SourceInfo {
        SourceInfo {
            id: source.id().to_owned(),
            source_type: source.info().name,
            is_volatile: source.common().volatile,
            attribution: source.data().attribution().filter(|attribution| !attribution.is_empty()),
            url: source.data().url().map(str::to_owned),
            source: source.clone(),
        }
    }
}

/// A layer of the map's style.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerInfo {
    pub id: String,
    /// The layer's [type](https://maplibre.org/maplibre-style-spec/layers/#type),
    /// such as `fill`.
    pub layer_type: &'static str,
    /// The source's ID, for a layer that takes one.
    pub source_id: Option<String>,
    /// The layer of a vector source that the layer draws.
    pub source_layer: Option<String>,
    pub min_zoom: Option<f64>,
    pub max_zoom: Option<f64>,
    pub visible: bool,
}

/// The work that prepares data for a style source, such as a GeoJSON
/// source's features, and makes the edit that gives the source the data.
pub type PrepareSourceData = Box<dyn FnOnce() -> Result<Edit, Error> + Send>;

/// The work that prepares a source's new data, as a source type's command
/// gives it to [`Map::set_style_source_data`], and where it runs.
pub enum NewSourceData {
    /// In the command, before later commands run.
    Now(PrepareSourceData),
    /// On a background thread.
    InBackground(PrepareSourceData),
}

/// What a map keeps for a source that [`Map::add_style_source`] added with
/// something to keep, until the source leaves the style.
struct KeptForSource {
    id: String,
    instance: SourceInstance,
    /// Held only to be dropped.
    _kept: Box<dyn Any + Send>,
}

/// What the map keeps for its style's sources, which it lets go of once
/// their source leaves the style.
#[derive(Default)]
pub(crate) struct KeptForSources(Vec<KeptForSource>);

/// The data of one source that a background thread prepares: the data being
/// prepared, and the newest data that waits to follow it.
struct PendingData {
    /// The source the data is for; a source that replaced it under its id
    /// doesn't take it.
    instance: SourceInstance,
    building: Option<Building>,
    waiting: Option<(PrepareSourceData, Resolver<Applied>)>,
}

/// Data being prepared: its number, and its completion, which resolves when
/// the data is installed or when something replaced the source's data
/// first.
struct Building {
    build: u64,
    resolver: Resolver<Applied>,
}

/// The sources whose new data is still being prepared, by source ID.
#[derive(Default)]
pub(crate) struct SourceDataUpdates(HashMap<String, PendingData>);

/// Lets go of what the map keeps for the sources that left the style, and
/// cancels the data being prepared or waiting for them. What the map kept drops from the outbox, so before
/// the completions that follow, and with the runtime's state unborrowed.
pub(crate) fn reconcile(entry: &mut MapEntry, outbox: &mut Outbox) {
    let map = &entry.core;
    let current = |id: &str, instance: SourceInstance| current_source(map, id, instance);
    let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut entry.kept_for_sources.0)
        .into_iter()
        .partition(|held| current(&held.id, held.instance).is_some());
    entry.kept_for_sources.0 = kept;
    if !gone.is_empty() {
        outbox.push(move || drop(gone));
    }
    entry.source_data_updates.0.retain(|id, update| {
        let present = current(id, update.instance).is_some();
        if !present {
            let cancelled = || Err(Error::cancelled("the source left the style before its data was ready"));
            if let Some(building) = update.building.take() {
                outbox.resolve(building.resolver, cancelled());
            }
            if let Some((_, resolver)) = update.waiting.take() {
                outbox.resolve(resolver, cancelled());
            }
        }
        present
    });
}

/// Lets go of everything a released map's sources hold, before its release
/// completes.
pub(crate) fn release(entry: &mut MapEntry, outbox: &mut Outbox) {
    let held = std::mem::take(&mut entry.kept_for_sources.0);
    if !held.is_empty() {
        outbox.push(move || drop(held));
    }
    for (_, mut update) in entry.source_data_updates.0.drain() {
        let cancelled = || Err(Error::cancelled("the map was released before the data was ready"));
        if let Some(building) = update.building.take() {
            outbox.resolve(building.resolver, cancelled());
        }
        if let Some((_, resolver)) = update.waiting.take() {
            outbox.resolve(resolver, cancelled());
        }
    }
}

fn quote(id: &str) -> String {
    format!("\"{id}\"")
}

fn require_loaded(map: &crate::map::MapCore) -> Result<(), Error> {
    if map.style_document_loaded() { Ok(()) } else { Err(style_not_loaded()) }
}

fn source_not_found(id: &str) -> Error {
    Error::not_found(format!("source {} does not exist", quote(id)))
}

fn find_source<'a>(map: &'a crate::map::MapCore, id: &str) -> Result<&'a Source, Error> {
    map.source(id).ok_or_else(|| source_not_found(id))
}

/// The data of `source`, a `T` source. Fails with `InvalidArgument` for a
/// source of another type, which isn't `kind`, as in `source "water" is not
/// a GeoJSON source`.
fn typed<'a, T: StyleSourceType>(source: &'a Source, kind: &str) -> Result<&'a SourceData<T>, Error> {
    SourceData::recover(source)
        .ok_or_else(|| Error::invalid_argument(format!("source {} is not {kind}", quote(source.id()))))
}

/// The source `id`, if it's still the source `instance`.
fn current_source<'a>(map: &'a crate::map::MapCore, id: &str, instance: SourceInstance) -> Option<&'a Source> {
    map.source(id).filter(|source| source.instance() == instance)
}

fn require_new_source(map: &crate::map::MapCore, id: &str) -> Result<(), Error> {
    require_loaded(map)?;
    if map.source(id).is_some() {
        return Err(Error::already_exists(format!("source {} already exists", quote(id))));
    }
    Ok(())
}

fn layer_info(layer: &mln_style::Layer) -> LayerInfo {
    LayerInfo {
        id: layer.get_id().to_owned(),
        layer_type: layer.info().name,
        source_id: Some(layer.get_source_id()).filter(|id| !id.is_empty()).map(str::to_owned),
        source_layer: Some(layer.get_source_layer()).filter(|id| !id.is_empty()).map(str::to_owned),
        min_zoom: Some(f64::from(layer.get_min_zoom())).filter(|zoom| zoom.is_finite()),
        max_zoom: Some(f64::from(layer.get_max_zoom())).filter(|zoom| zoom.is_finite()),
        visible: layer.get_visibility() != mln_style_types::VisibilityType::None,
    }
}

impl Map {
    // MARK: - Global state

    /// Sets the value `name` that the style's `global-state` expressions
    /// read, from JSON. See `Style::set_global_state_property_json`.
    pub fn set_global_state_property(
        &self,
        name: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (name, json) = (name.into(), json.into());
        self.command(move |map| map.edit_loaded_style(|style| style.set_global_state_property_json(&name, &json)))
    }

    /// The style's global state as a JSON object.
    pub fn global_state(&self) -> Result<Completion<String>, Error> {
        self.query(|map| Ok(map.style().global_state_json()))
    }

    // MARK: - Sources

    /// A source of the style, or `None` when the style has no such source.
    pub fn style_source(&self, source_id: impl Into<String>) -> Result<Completion<Option<SourceInfo>>, Error> {
        let source_id = source_id.into();
        self.query(move |map| Ok(map.source(&source_id).map(SourceInfo::of)))
    }

    /// The style's sources, in style order.
    pub fn style_sources(&self) -> Result<Completion<Vec<SourceInfo>>, Error> {
        self.query(|map| Ok(map.sources().map(SourceInfo::of).collect()))
    }

    /// Sets whether a source keeps its tiles out of the persistent cache.
    pub fn set_style_source_volatile(
        &self,
        source_id: impl Into<String>,
        is_volatile: bool,
    ) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.command(move |map| {
            require_loaded(map)?;
            let mut common = *find_source(map, &source_id)?.common();
            common.volatile = is_volatile;
            map.edit_style(|style| style.apply(Edit::SetSourceCommon { id: source_id, common }))
        })
    }

    /// Adds the source that `make` makes, for a source that isn't JSON, such
    /// as one that a source type's own command adds. `make` runs on the
    /// runtime's thread, in the command's place among the map's commands,
    /// with the map, which gives its thread pool and registry.
    ///
    /// The map keeps `kept`, such as the host's callbacks that the source
    /// refers to weakly, until the source leaves the style by its removal, a
    /// style load or the map's release. `kept` drops on the runtime's thread
    /// with the runtime's state unborrowed, before the completion of what
    /// removed the source, or of this command when it fails.
    ///
    /// Completes with what `make` fails with,
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) while the style
    /// isn't loaded, `AlreadyExists` when the style has a source with the
    /// made source's ID, and `InvalidArgument` when the made source is of a
    /// type the map's registry lacks.
    pub fn add_style_source(
        &self,
        make: impl FnOnce(&crate::map::MapCore) -> Result<Source, Error> + Send + 'static,
        kept: Option<Box<dyn Any + Send>>,
    ) -> Result<Completion<Applied>, Error> {
        self.send(
            move |entry, outbox| match add_source(&mut entry.core, make) {
                Ok((id, instance)) => {
                    if let Some(kept) = kept {
                        entry.kept_for_sources.0.push(KeptForSource { id, instance, _kept: kept });
                    }
                    Ok(())
                }
                Err(error) => {
                    if let Some(kept) = kept {
                        outbox.push(move || drop(kept));
                    }
                    Err(error)
                }
            },
            |(), entry| entry.applied(),
        )
    }

    /// Applies `edit` to the style, such as an edit that a source type makes
    /// for its sources. Completes with what the edit fails with, and with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) while the style isn't
    /// loaded.
    ///
    /// An edit that replaces a source's data, such as a new URL, grows its
    /// [`load`](mln_style::source_type::SourceData::load): the data that a
    /// [`set_style_source_data`](Self::set_style_source_data) sent before
    /// and that waits to be prepared then completes with
    /// [`ErrorKind::Cancelled`](mln_util::ErrorKind), and the data being
    /// prepared finds out once it's ready.
    pub fn apply_style_edit(&self, edit: Edit) -> Result<Completion<Applied>, Error> {
        self.send(
            move |entry, outbox| {
                let waiting = waiting_loads(entry);
                entry.core.edit_loaded_style(|style| style.apply(edit))?;
                for (id, load) in waiting {
                    cancel_replaced(entry, &id, load, outbox);
                }
                Ok(())
            },
            |(), entry| entry.applied(),
        )
    }

    /// Runs `act` on the data of the style's source `source_id`, a `T`
    /// source, on the runtime's thread, in the command's place among the
    /// map's commands: a command that acts through the source without
    /// editing the style, such as an answer to a custom source's request
    /// for a tile.
    ///
    /// Completes with [`ErrorKind::NotFound`](mln_util::ErrorKind) when the
    /// style has no such source, with `InvalidArgument` when it isn't a `T`
    /// source, as in `source "water" is not {kind}`, and with what `act`
    /// fails with.
    pub fn command_style_source<T: StyleSourceType>(
        &self,
        source_id: impl Into<String>,
        kind: &'static str,
        act: impl FnOnce(&SourceData<T>) -> Result<(), Error> + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.command(move |map| act(typed(find_source(map, &source_id)?, kind)?))
    }

    /// Reads the data of the style's source `source_id`, a `T` source, with
    /// `read` on the runtime's thread, in the query's place among the map's
    /// commands. Completes as
    /// [`command_style_source`](Self::command_style_source) does.
    pub fn query_style_source<T: StyleSourceType, R: Send + 'static>(
        &self,
        source_id: impl Into<String>,
        kind: &'static str,
        read: impl FnOnce(&SourceData<T>) -> Result<R, Error> + Send + 'static,
    ) -> Result<Completion<R>, Error> {
        let source_id = source_id.into();
        self.query(move |map| read(typed(find_source(map, &source_id)?, kind)?))
    }

    /// Gives the source `source_id`, a `T` source, new data. On the
    /// runtime's thread, in the command's place among the map's commands,
    /// `data` reads the source's data and makes the work that prepares the
    /// new data, which runs in the command or on a background thread. The
    /// command completes once the source holds the data. Later commands
    /// don't wait for work in the background. While the source is still
    /// preparing earlier data in the background, only the newest data waits
    /// to follow it.
    ///
    /// Completes with [`ErrorKind::InvalidState`](mln_util::ErrorKind) while
    /// the style isn't loaded, with `NotFound` and `InvalidArgument` as
    /// [`command_style_source`](Self::command_style_source) does, with what
    /// `data` or the preparation fails with, and with `Cancelled` when newer
    /// data replaces it while it waits, or when an edit that replaces the
    /// source's data (that grows its `load`), the source's removal, a style
    /// load or the map's release comes before the data is ready.
    pub fn set_style_source_data<T: StyleSourceType>(
        &self,
        source_id: impl Into<String>,
        kind: &'static str,
        data: impl FnOnce(&SourceData<T>) -> Result<NewSourceData, Error> + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.post(move |core, outbox, id, sequence, resolver| {
            if !core.maps.contains_key(&id) {
                return outbox.resolve(resolver, Err(missing_map()));
            }
            let background = core.background();
            apply_command(
                core,
                id,
                sequence,
                outbox,
                move |entry, outbox| match queue_data(entry, id, &source_id, kind, data, resolver, background, outbox) {
                    Ok(installed) => installed,
                    Err((error, resolver)) => {
                        outbox.resolve(resolver, Err(error));
                        None
                    }
                },
                |_, installed, applied, outbox| {
                    if let Some(resolver) = installed {
                        outbox.resolve(resolver, Ok(applied));
                    }
                },
            );
        })
    }

    // MARK: - Images

    /// Adds `image` to the style, or replaces the image with its ID.
    pub fn set_style_image(&self, image: StyleImage) -> Result<Completion<Applied>, Error> {
        self.command(move |map| map.edit_loaded_style(|style| style.apply(Edit::AddImage(Arc::new(image)))))
    }

    /// Removes an image from the style. Completes with
    /// [`ErrorKind::NotFound`](mln_util::ErrorKind) when the style has no
    /// such image.
    pub fn remove_style_image(&self, image_id: impl Into<String>) -> Result<Completion<Applied>, Error> {
        let image_id = image_id.into();
        self.command(move |map| {
            map.edit_loaded_style(|style| {
                if style.get_image(&image_id).is_none() {
                    return Err(Error::not_found(format!("style image {} does not exist", quote(&image_id))));
                }
                style.apply(Edit::RemoveImage(image_id))
            })
        })
    }

    /// An image of the style, or `None` when the style has no such image.
    pub fn style_image(&self, image_id: impl Into<String>) -> Result<Completion<Option<Arc<StyleImage>>>, Error> {
        let image_id = image_id.into();
        self.query(move |map| Ok(map.style().get_image(&image_id).cloned()))
    }

    // MARK: - Layers

    /// A layer of the style, or `None` when the style has no such layer.
    pub fn style_layer(&self, layer_id: impl Into<String>) -> Result<Completion<Option<LayerInfo>>, Error> {
        let layer_id = layer_id.into();
        self.query(move |map| Ok(map.style().get_layer(&layer_id).map(layer_info)))
    }

    /// The style's layers, from bottom to top.
    pub fn style_layers(&self) -> Result<Completion<Vec<LayerInfo>>, Error> {
        self.query(|map| Ok(map.style().get_layers().map(layer_info).collect()))
    }

    /// Changes the [`Data`](mln_style::layer_type::StyleLayerType::Data) of
    /// the style's layer `layer_id`, a layer of the type `S`, with `edit`:
    /// a new version of the style, which the renderer takes without laying
    /// the layer out again or restarting its transitions. A layer keeps its
    /// data when a new style with a layer of its id and type replaces the
    /// style.
    ///
    /// Completes with [`ErrorKind::InvalidState`](mln_util::ErrorKind)
    /// while the style isn't loaded, with `NotFound` when the style has no
    /// such layer of the type `S`, and with what `edit` fails with.
    pub fn edit_style_layer_data<S: StyleLayerType>(
        &self,
        layer_id: impl Into<String>,
        edit: impl FnOnce(&mut S::Data) -> Result<(), Error> + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        let layer_id = layer_id.into();
        self.command(move |map| map.edit_loaded_style(|style| style.edit_layer_data::<S>(&layer_id, edit)))
    }

    /// Moves a layer below `before_layer_id`, or above every layer. See
    /// `Style::move_layer` for the errors.
    pub fn move_style_layer(
        &self,
        layer_id: impl Into<String>,
        before_layer_id: Option<String>,
    ) -> Result<Completion<Applied>, Error> {
        let layer_id = layer_id.into();
        self.command(move |map| map.edit_loaded_style(|style| style.move_layer(&layer_id, before_layer_id.as_deref())))
    }

    // MARK: - Transitions

    /// Sets the default transition of the style's paint properties.
    pub fn set_style_transition_options(&self, options: TransitionOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| map.edit_loaded_style(|style| style.apply(Edit::SetTransition(options))))
    }

    /// The default transition of the style's paint properties.
    pub fn style_transition_options(&self) -> Result<Completion<TransitionOptions>, Error> {
        self.query(|map| Ok(map.style().get_transition_options()))
    }
}

/// Adds the source that `make` makes, and returns its ID and instance.
fn add_source(
    map: &mut crate::map::MapCore,
    make: impl FnOnce(&crate::map::MapCore) -> Result<Source, Error>,
) -> Result<(String, SourceInstance), Error> {
    let source = make(map)?;
    let (id, instance) = (source.id().to_owned(), source.instance());
    require_new_source(map, &id)?;
    map.add_source(source)?;
    Ok((id, instance))
}

/// The sources whose data waits to be prepared, with their `load`, which
/// [`cancel_replaced`] compares after an edit.
fn waiting_loads(entry: &MapEntry) -> Vec<(String, u64)> {
    let updates = entry.source_data_updates.0.iter().filter(|(_, update)| update.waiting.is_some());
    updates
        .filter_map(|(id, update)| Some((id.clone(), current_source(&entry.core, id, update.instance)?.data().load())))
        .collect()
}

/// Cancels the data that waits for the source `id` if its `load` is no
/// longer `load`: an edit replaced the source's data.
fn cancel_replaced(entry: &mut MapEntry, id: &str, load: u64, outbox: &mut Outbox) {
    let Some(update) = entry.source_data_updates.0.get_mut(id) else { return };
    let current = current_source(&entry.core, id, update.instance).map(|source| source.data().load());
    if current != Some(load)
        && let Some((_, resolver)) = update.waiting.take()
    {
        outbox.resolve(resolver, Err(Error::cancelled("an edit replaced the source's data")));
    }
}

/// Takes new data for the source `source_id`, a `T` source, whose work
/// `data` makes from its data: prepares it now, and returns its resolver
/// for the command to resolve once it ran; or starts preparing it in the
/// background, or leaves it waiting behind the data being prepared. An error returns
/// the resolver with it.
#[expect(clippy::too_many_arguments, reason = "one command's state, taken apart")]
fn queue_data<T: StyleSourceType>(
    entry: &mut MapEntry,
    map_id: MapId,
    source_id: &str,
    kind: &str,
    data: impl FnOnce(&SourceData<T>) -> Result<NewSourceData, Error>,
    resolver: Resolver<Applied>,
    background: Background,
    outbox: &mut Outbox,
) -> Result<Option<Resolver<Applied>>, (Error, Resolver<Applied>)> {
    let made = (|| {
        require_loaded(&entry.core)?;
        let source = find_source(&entry.core, source_id)?;
        Ok((source, data(typed(source, kind)?)?))
    })();
    let (source, work) = match made {
        Ok((source, NewSourceData::InBackground(work))) => (source, work),
        Ok((_, NewSourceData::Now(work))) => {
            return match work().and_then(|edit| entry.core.edit_style(|style| style.apply(edit))) {
                Ok(()) => Ok(Some(resolver)),
                Err(error) => Err((error, resolver)),
            };
        }
        Err(error) => return Err((error, resolver)),
    };
    let instance = source.instance();
    let update = entry.source_data_updates.0.entry(source_id.to_owned()).or_insert_with(|| PendingData {
        instance,
        building: None,
        waiting: None,
    });
    if update.building.is_none() {
        update.instance = instance;
        start_data(update, map_id, source, work, resolver, background);
    } else if let Some((_, replaced)) = update.waiting.replace((work, resolver)) {
        outbox.resolve(replaced, Err(Error::cancelled("newer data for the source replaced it")));
    }
    Ok(None)
}

/// Runs `work`, which prepares data for `update`'s source, `source`, on a
/// background thread, then installs the data from the runtime's thread.
fn start_data(
    update: &mut PendingData,
    map_id: MapId,
    source: &Source,
    work: PrepareSourceData,
    resolver: Resolver<Applied>,
    background: Background,
) {
    // Numbers each preparation, so one that something replaced finds no
    // completion of its own.
    static BUILDS: AtomicU64 = AtomicU64::new(0);
    let build = BUILDS.fetch_add(1, Ordering::Relaxed);
    let load = source.data().load();
    update.building = Some(Building { build, resolver });
    let source_id = source.id().to_owned();
    let next = background.clone();
    background.pool.spawn(move || {
        let edit = work();
        post(&background.queue, move |core, outbox| {
            install_data(core, map_id, &source_id, (build, load), edit, next, outbox);
        });
    });
}

/// Installs data that a background thread prepared, unless something
/// replaced the source's data while it was prepared, then starts the data
/// that waits.
fn install_data(
    core: &mut RuntimeCore,
    map_id: MapId,
    source_id: &str,
    (build, load): (u64, u64),
    edit: Result<Edit, Error>,
    background: Background,
    outbox: &mut Outbox,
) {
    flush(core, map_id, outbox);
    let Some(entry) = core.maps.get_mut(&map_id) else { return };
    let Some(update) = entry.source_data_updates.0.get_mut(source_id) else { return };
    let Some(Building { resolver, .. }) = update.building.take_if(|building| building.build == build) else {
        return;
    };
    let current = current_source(&entry.core, source_id, update.instance).map(|source| source.data().load());
    let installed = match edit {
        Err(error) => Err(error),
        Ok(_) if current != Some(load) => {
            Err(Error::cancelled("the source's data was replaced before this data was ready"))
        }
        Ok(edit) => entry.core.edit_style(|style| style.apply(edit)),
    };
    match (update.waiting.take(), current_source(&entry.core, source_id, update.instance)) {
        (Some((work, next)), Some(source)) => start_data(update, map_id, source, work, next, background),
        (Some((_, next)), None) => outbox.resolve(next, Err(Error::cancelled("the source left the style"))),
        (None, _) => {
            entry.source_data_updates.0.remove(source_id);
        }
    }
    flush(core, map_id, outbox);
    let Some(entry) = core.maps.get(&map_id) else { return };
    outbox.resolve(resolver, installed.map(|()| entry.applied()));
}
