// `GeoJSONSource` is a handle (see source.rs): `Clone` shares the source.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use geojson::GeoJson;
use mln_actor::RunLoop;
use mln_actor::scheduler::{self, SchedulerPtr};
use mln_storage::{FileSource, FileSourceExt, Request, Resource, Response};
use mln_util::logging::{Event, Log};

pub use super::geojson::{
    ClusterProperties, Features, GeoJSONData, GeoJSONDataBuilder, GeoJSONOptions, TileFeatures, create_geojson_data,
};
use super::geojson::{GeoJsonSourceOptions, GeoJsonSourceType};
use crate::conversion::Error;
use crate::conversion::geojson::convert_json_geojson;
use crate::source::{Source, SourceBase};
use crate::source_type::{self, SourceData};

struct Inner {
    base: SourceBase,
    req: RefCell<Option<Request>>,
    request_generation: Cell<u64>,
    sequenced_scheduler: SchedulerPtr,
}

#[derive(Clone)]
pub struct GeoJSONSource {
    inner: Rc<Inner>,
}

impl std::fmt::Debug for GeoJSONSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoJSONSource").field("base", &self.inner.base).finish_non_exhaustive()
    }
}

impl GeoJSONSource {
    pub fn new(id: impl Into<String>, options: Arc<GeoJSONOptions>) -> Self {
        let data = SourceData::<GeoJsonSourceType>::new(GeoJsonSourceOptions::new(options));
        GeoJSONSource {
            inner: Rc::new(Inner {
                base: SourceBase::new(source_type::Source::new(&id.into(), data)),
                req: RefCell::new(None),
                request_generation: Cell::new(0),
                sequenced_scheduler: scheduler::get_sequenced(),
            }),
        }
    }

    /// Creates a source with the default options.
    pub fn with_default_options(id: impl Into<String>) -> Self {
        Self::new(id, GeoJSONOptions::default_options())
    }

    fn read<R>(&self, f: impl FnOnce(&SourceData<GeoJsonSourceType>) -> R) -> Option<R> {
        SourceData::recover(&self.inner.base.base_impl()).map(f)
    }

    /// Records the source's data as `f` changes it.
    fn update(&self, f: impl FnOnce(&mut SourceData<GeoJsonSourceType>)) {
        if let Some(record) = self.inner.base.base_impl().map_data(f) {
            self.inner.base.set_base_impl(record);
        }
    }

    /// Sets the URL of a GeoJSON file to fetch.
    pub fn set_url(&self, url: &str) {
        self.update(|data| {
            data.options.url = Some(url.to_owned());
            data.load += 1;
        });
        self.drop_building_response();

        // Signal that the source description needs a reload
        if self.loaded() || self.inner.req.borrow().is_some() {
            self.inner.base.set_loaded(false);
            *self.inner.req.borrow_mut() = None;
            self.inner.base.observer().on_source_description_changed(self);
        }
    }

    /// Sets the source's data. Fails as [`create_geojson_data`] does.
    pub fn set_geojson(&self, geojson: GeoJson) -> Result<(), mln_util::Error> {
        let data = self.data_builder().build(geojson)?;
        self.set_geojson_data(Some(data));
        Ok(())
    }

    /// What builds data for this source with its options, on any thread,
    /// for [`set_geojson_data`](Self::set_geojson_data) to take.
    #[must_use]
    pub fn data_builder(&self) -> GeoJSONDataBuilder {
        GeoJSONDataBuilder::new(self.get_options(), Arc::clone(&self.inner.sequenced_scheduler))
    }

    /// A count that goes up each time new data, a URL, or a URL's response
    /// replaces the source's data or starts to: data built while it stays
    /// the same is still the newest.
    #[must_use]
    pub fn data_generation(&self) -> u64 {
        self.inner.request_generation.get()
    }

    pub fn set_geojson_data(&self, geojson_data: Option<Arc<dyn GeoJSONData>>) {
        *self.inner.req.borrow_mut() = None;
        self.drop_building_response();
        self.update(|data| {
            data.description = geojson_data;
            data.load += 1;
        });
        self.inner.base.observer().on_source_changed(self);
    }

    /// Drops the result of a response that is still building in the
    /// background.
    fn drop_building_response(&self) {
        self.inner.request_generation.set(self.inner.request_generation.get() + 1);
    }

    #[must_use]
    pub fn get_url(&self) -> Option<String> {
        self.read(|data| data.options.url.clone()).flatten()
    }

    #[must_use]
    pub fn get_options(&self) -> Arc<GeoJSONOptions> {
        self.read(|data| Arc::clone(&data.options.options)).unwrap_or_else(GeoJSONOptions::default_options)
    }

    #[must_use]
    pub fn is_update_synchronous(&self) -> bool {
        self.read(|data| data.options.synchronous_update).unwrap_or_default()
    }

    /// Replaces the `synchronous_update` option, on or off. The renderer
    /// recreates the source's tiles in the new mode.
    pub fn set_override_synchronous_update(&self, value: bool) {
        if self.is_update_synchronous() == value {
            return;
        }
        self.update(|data| data.options.synchronous_update = value);
        self.inner.base.observer().on_source_changed(self);
    }
}

/// The part of `loadDescription`'s response handler that runs on the
/// requesting thread, `run_loop`'s, which parses the data on its pool.
fn on_description_response(weak: &Weak<Inner>, res: &Response, run_loop: &RunLoop) {
    let Some(inner) = weak.upgrade() else {
        return;
    };
    let this = GeoJSONSource { inner };
    if let Some(error) = &res.error {
        this.inner.base.observer().on_source_error(&this, mln_util::Error::native(error.message.clone()));
    } else if res.not_modified {
        // return
    } else if res.no_content {
        this.inner.base.observer().on_source_error(&this, mln_util::Error::native("unexpectedly empty GeoJSON"));
    } else {
        // Note: This task appears to be safe enough to schedule on the
        // generic background queue. This task does not reference other
        // objects who's lifetimes are coupled with a map.
        let options = this.get_options();
        let data = res.data.clone();
        let seq_scheduler = Arc::clone(&this.inner.sequenced_scheduler);
        let generation = this.inner.request_generation.get() + 1;
        this.inner.request_generation.set(generation);
        let weak = Rc::downgrade(&this.inner);
        scheduler::schedule_and_reply_value(
            &*run_loop.pool(),
            None,
            &run_loop.scheduler(),
            /* makeImplInBackground */
            move || -> Option<Arc<dyn GeoJSONData>> {
                let data = data.expect("response data");
                let mut error = Error::default();
                let mut geojson_data = None;
                if let Some(geojson) = convert_json_geojson(&data, &mut error) {
                    match create_geojson_data(geojson, seq_scheduler, &options) {
                        Ok(created) => geojson_data = Some(created),
                        // C++ lets the Supercluster exception
                        // escape the background task.
                        Err(e) => Log::error(Event::ParseStyle, format!("Failed to parse GeoJSON data: {e}")),
                    }
                } else {
                    // Create an empty GeoJSON VT object to make sure we're
                    // not infinitely waiting for tiles to load.
                    Log::error(Event::ParseStyle, format!("Failed to parse GeoJSON data: {}", error.message));
                }
                geojson_data
            },
            /* onImplReady */
            move |geojson_data: Option<Arc<dyn GeoJSONData>>| {
                if let Some(inner) = weak.upgrade() {
                    // If a new request is being processed, ignore this data.
                    if generation == inner.request_generation.get() {
                        let this = GeoJSONSource { inner };
                        // The record is made here, from the current one, so
                        // that it keeps what changed during the build, such
                        // as the synchronous-update override.
                        this.update(|data| data.description = geojson_data);
                        this.inner.base.set_loaded(true);
                        this.inner.base.observer().on_source_loaded(&this);
                    }
                }
            },
        );
    }
}

impl Source for GeoJSONSource {
    fn base(&self) -> &SourceBase {
        &self.inner.base
    }

    fn load_description(&self, file_source: &dyn FileSource, run_loop: &RunLoop) {
        let Some(url) = self.get_url() else {
            self.inner.base.set_loaded(true);
            return;
        };

        if self.inner.req.borrow().is_some() {
            return;
        }

        let (weak, callback_loop) = (Rc::downgrade(&self.inner), run_loop.clone());
        let req = file_source.request(
            &run_loop.scheduler(),
            &Resource::source(&url),
            Box::new(move |res: Response| on_description_response(&weak, &res, &callback_loop)),
        );
        *self.inner.req.borrow_mut() = Some(req);
    }
}
