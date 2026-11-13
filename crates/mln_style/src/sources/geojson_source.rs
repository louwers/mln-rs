// `GeoJSONSource` is a handle (see source.rs): `Clone` shares the source.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::{Arc, OnceLock};

use feature::Feature;
use geojson::GeoJSON;
use mln_actor::scheduler::{self, SchedulerPtr};
use mln_storage::{AsyncRequest, FileSource, Resource, Response};
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use mln_vec_map::VecMap;

use super::geojson_source_impl::{self, Impl};
use crate::conversion::Error;
use crate::conversion::geojson::convert_json_geojson;
use crate::expression::Expression;
use crate::layer::LayerTypeInfo;
use crate::layer::layer_type_info::TileKind;
use crate::source::{Source, SourceBase, downcast_arc};
use crate::source_impl;

pub type ClusterExpression = (Arc<dyn Expression>, Arc<dyn Expression>);
pub type ClusterProperties = VecMap<String, ClusterExpression>;

#[derive(Clone)]
pub struct GeoJSONOptions {
    // GeoJSON-VT options
    pub minzoom: u8,
    pub maxzoom: u8,
    pub tile_size: u16,
    pub buffer: u16,
    pub tolerance: f64,
    pub line_metrics: bool,

    // Supercluster options
    pub cluster: bool,
    pub cluster_radius: u16,
    pub cluster_max_zoom: u8,
    pub cluster_min_points: usize,
    pub cluster_properties: ClusterProperties,

    // Update options
    pub synchronous_update: bool,

    /// The style's attribution for the source, reported by
    /// `Source::get_attribution`.
    pub attribution: Option<String>,
}

impl Default for GeoJSONOptions {
    fn default() -> Self {
        GeoJSONOptions {
            minzoom: 0,
            maxzoom: 18,
            tile_size: constants::TILE_SIZE_I,
            buffer: 128,
            tolerance: 0.375,
            line_metrics: false,
            cluster: false,
            cluster_radius: 50,
            cluster_max_zoom: 17,
            cluster_min_points: 2,
            cluster_properties: ClusterProperties::new(),
            synchronous_update: false,
            attribution: None,
        }
    }
}

impl std::fmt::Debug for GeoJSONOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoJSONOptions")
            .field("minzoom", &self.minzoom)
            .field("maxzoom", &self.maxzoom)
            .field("tile_size", &self.tile_size)
            .field("buffer", &self.buffer)
            .field("tolerance", &self.tolerance)
            .field("line_metrics", &self.line_metrics)
            .field("cluster", &self.cluster)
            .field("cluster_radius", &self.cluster_radius)
            .field("cluster_max_zoom", &self.cluster_max_zoom)
            .field("cluster_min_points", &self.cluster_min_points)
            .field("cluster_properties", &self.cluster_properties.keys().collect::<Vec<_>>())
            .field("synchronous_update", &self.synchronous_update)
            .field("attribution", &self.attribution)
            .finish()
    }
}

impl GeoJSONOptions {
    /// The default options, as one shared instance.
    pub fn default_options() -> Arc<GeoJSONOptions> {
        static OPTIONS: OnceLock<Arc<GeoJSONOptions>> = OnceLock::new();
        OPTIONS.get_or_init(|| Arc::new(GeoJSONOptions::default())).clone()
    }
}

pub type TileFeatures = Vec<Feature<i16>>;
pub type Features = Vec<Feature<f64>>;

pub trait GeoJSONData: Send + Sync {
    /// Gets the features of tile `id`. `callback` runs on the calling thread,
    /// either right away or later from its `RunLoop`.
    fn get_tile(&self, id: &CanonicalTileID, callback: Box<dyn FnOnce(TileFeatures)>, run_synchronously: bool);

    // SuperclusterData: data that isn't clustered has no clusters, and
    // gives empty results. A cluster ID the data doesn't have fails with
    // `ErrorKind::NotFound`.
    fn get_children(&self, cluster_id: u32) -> Result<Features, mln_util::Error>;
    fn get_leaves(&self, cluster_id: u32, limit: u32, offset: u32) -> Result<Features, mln_util::Error>;
    fn get_cluster_expansion_zoom(&self, cluster_id: u32) -> Result<u8, mln_util::Error>;
}

/// Prepares GeoJSON for tiling. GeoJSON that can't be clustered, such as a
/// non-point feature, fails with
/// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind).
pub fn create_geojson_data(
    geojson: &GeoJSON,
    sequenced_scheduler: SchedulerPtr,
    options: &Arc<GeoJSONOptions>,
) -> Result<Arc<dyn GeoJSONData>, mln_util::Error> {
    geojson_source_impl::create(geojson, sequenced_scheduler, options)
}

/// Builds the data of a [`GeoJSONSource`] with the source's options. It is
/// `Send`, so the work can leave the source's thread.
#[derive(Clone)]
pub struct GeoJSONDataBuilder {
    options: Arc<GeoJSONOptions>,
    sequenced_scheduler: SchedulerPtr,
}

impl GeoJSONDataBuilder {
    #[must_use]
    pub fn options(&self) -> &Arc<GeoJSONOptions> {
        &self.options
    }

    /// Fails as [`create_geojson_data`] does.
    pub fn build(&self, geojson: &GeoJSON) -> Result<Arc<dyn GeoJSONData>, mln_util::Error> {
        create_geojson_data(geojson, self.sequenced_scheduler.clone(), &self.options)
    }
}

struct Inner {
    base: SourceBase,
    url: RefCell<Option<String>>,
    req: RefCell<Option<Box<dyn AsyncRequest>>>,
    request_generation: Cell<u64>,
    sequenced_scheduler: SchedulerPtr,
    /// Keeps `Scheduler::GetBackground()` alive while a description loads
    /// (the C++ background pool lives as long as any of its users).
    background: SchedulerPtr,
}

#[derive(Clone)]
pub struct GeoJSONSource {
    inner: Rc<Inner>,
}

impl std::fmt::Debug for GeoJSONSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoJSONSource")
            .field("base", &self.inner.base)
            .field("url", &self.inner.url.borrow())
            .finish_non_exhaustive()
    }
}

impl GeoJSONSource {
    pub fn new(id: impl Into<String>, options: Arc<GeoJSONOptions>) -> Self {
        let impl_: Arc<dyn source_impl::Impl> = Arc::new(Impl::new(id.into(), options));
        GeoJSONSource {
            inner: Rc::new(Inner {
                base: SourceBase::new(impl_),
                url: RefCell::new(None),
                req: RefCell::new(None),
                request_generation: Cell::new(0),
                sequenced_scheduler: scheduler::get_sequenced(),
                background: scheduler::get_background(),
            }),
        }
    }

    /// Creates a source with the default options.
    pub fn with_default_options(id: impl Into<String>) -> Self {
        Self::new(id, GeoJSONOptions::default_options())
    }

    #[must_use]
    pub fn impl_(&self) -> Arc<Impl> {
        downcast_arc::<Impl>(&self.inner.base.base_impl())
    }

    /// Sets the URL of a GeoJSON file to fetch.
    pub fn set_url(&self, url: &str) {
        *self.inner.url.borrow_mut() = Some(url.to_owned());
        self.drop_building_response();

        // Signal that the source description needs a reload
        if self.loaded() || self.inner.req.borrow().is_some() {
            self.inner.base.set_loaded(false);
            *self.inner.req.borrow_mut() = None;
            self.inner.base.observer().on_source_description_changed(self);
        }
    }

    /// Sets the source's data. Fails as [`create_geojson_data`] does.
    pub fn set_geojson(&self, geojson: &GeoJSON) -> Result<(), mln_util::Error> {
        let data =
            create_geojson_data(geojson, self.inner.sequenced_scheduler.clone(), &self.impl_().get_options().clone())?;
        self.set_geojson_data(Some(data));
        Ok(())
    }

    /// What builds data for this source with its options, on any thread,
    /// for [`set_geojson_data`](Self::set_geojson_data) to take.
    #[must_use]
    pub fn data_builder(&self) -> GeoJSONDataBuilder {
        GeoJSONDataBuilder {
            options: self.impl_().get_options().clone(),
            sequenced_scheduler: self.inner.sequenced_scheduler.clone(),
        }
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
        let new_impl: Arc<dyn source_impl::Impl> = Arc::new(Impl::with_data(&self.impl_(), geojson_data));
        self.inner.base.set_base_impl(new_impl);
        self.inner.base.observer().on_source_changed(self);
    }

    /// Drops the result of a response that is still building in the
    /// background.
    fn drop_building_response(&self) {
        self.inner.request_generation.set(self.inner.request_generation.get() + 1);
    }

    #[must_use]
    pub fn get_url(&self) -> Option<String> {
        self.inner.url.borrow().clone()
    }

    #[must_use]
    pub fn get_options(&self) -> Arc<GeoJSONOptions> {
        self.impl_().get_options().clone()
    }

    #[must_use]
    pub fn is_update_synchronous(&self) -> bool {
        source_impl::Impl::is_update_synchronous(&*self.impl_())
    }

    /// Replaces the `synchronous_update` option, on or off. The renderer
    /// recreates the source's tiles in the new mode.
    pub fn set_override_synchronous_update(&self, value: bool) {
        if self.is_update_synchronous() == value {
            return;
        }
        let mut new_impl = (*self.impl_()).clone();
        new_impl.set_update_synchronous(value);
        self.inner.base.set_base_impl(Arc::new(new_impl));
        self.inner.base.observer().on_source_changed(self);
    }
}

/// The part of `loadDescription`'s response handler that runs on the
/// requesting thread.
fn on_description_response(weak: &Weak<Inner>, res: &Response) {
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
        let options = this.impl_().get_options().clone();
        let data = res.data.clone();
        let seq_scheduler = this.inner.sequenced_scheduler.clone();
        let generation = this.inner.request_generation.get() + 1;
        this.inner.request_generation.set(generation);
        let weak = Rc::downgrade(&this.inner);
        scheduler::schedule_and_reply_value(
            &*this.inner.background,
            SimpleIdentity::EMPTY,
            /* makeImplInBackground */
            move || -> Option<Arc<dyn GeoJSONData>> {
                let data = data.expect("response data");
                let mut error = Error::default();
                let mut geojson_data = None;
                if let Some(geojson) = convert_json_geojson(&data, &mut error) {
                    match create_geojson_data(&geojson, seq_scheduler, &options) {
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
                        // The impl is made here, from the current one, so
                        // that it keeps what changed during the build, such
                        // as the synchronous-update override.
                        let new_impl: Arc<dyn source_impl::Impl> =
                            Arc::new(Impl::with_data(&this.impl_(), geojson_data));
                        this.inner.base.set_base_impl(new_impl);
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

    fn load_description(&self, file_source: &dyn FileSource) {
        let url = self.inner.url.borrow().clone();
        let Some(url) = url else {
            self.inner.base.set_loaded(true);
            return;
        };

        if self.inner.req.borrow().is_some() {
            return;
        }

        let weak = Rc::downgrade(&self.inner);
        let req = file_source
            .request(&Resource::source(&url), Box::new(move |res: Response| on_description_response(&weak, &res)));
        *self.inner.req.borrow_mut() = Some(req);
    }

    fn supports_layer_type(&self, info: &LayerTypeInfo) -> bool {
        TileKind::Geometry as u8 == info.tile_kind as u8
    }

    fn create_mutable(&self) -> Box<dyn source_impl::Impl> {
        Box::new((*self.impl_()).clone())
    }
}

// Debug is not a supertrait, for the reason at `impl Debug for dyn Layer`
// (layer.rs).
impl std::fmt::Debug for dyn GeoJSONData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoJSONData").finish_non_exhaustive()
    }
}
