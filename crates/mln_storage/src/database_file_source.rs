// Ports include/mln/storage/database_file_source.hpp and
// platform/default/src/mln/storage/database_file_source.cpp.
//
// The database runs on its own thread (`util::Thread<DatabaseFileSourceThread>`
// "DatabaseFileSource"); requests reach it as actor messages and the
// response goes back to the requesting thread's `FileSourceRequest`.
//
// C++ `DatabaseFileSourceThread` reads the tile server options from the
// online file source (`onlineFileSource_->getResourceOptions()`); so does
// the port, falling back to the ResourceOptions only when there is no
// Network file source (C++ would dereference null).
//
// TODO(port): the offline region API (list/get/create/merge/update/delete/
// invalidate regions, region observers and download state, region status;
// OfflineDownload lives in //crates/mln_storage_offline), resetDatabase,
// packDatabase, invalidate/clear ambient cache, setMaximumAmbientCacheSize.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use feature::Value;
use mln_actor::{ActorRef, Thread, scheduler};
use mln_util::client_options::ClientOptions;
use mln_util::constants::{ASSET_PROTOCOL, FILE_PROTOCOL};
use mln_util::logging::{Event, Log};
use mln_util::sync::MutexExt;
use mln_util::tile_server_options::TileServerOptions;

use crate::async_request::AsyncRequest;
use crate::file_source::{Callback, FileSource, FileSourceType, READ_ONLY_MODE_KEY};
use crate::file_source_manager::FileSourceManager;
use crate::file_source_request::{FileSourceRequest, FileSourceRequestState};
use crate::offline_database::OfflineDatabase;
use crate::resource::{LoadingMethod, Resource, StoragePolicy};
use crate::resource_options::ResourceOptions;
use crate::response::{Error, Reason, Response};

/// `DatabaseFileSourceThread`: owns the OfflineDatabase on the database
/// thread.
pub struct DatabaseFileSourceThread {
    db: RefCell<OfflineDatabase>,
    #[allow(dead_code)]
    online_file_source: Option<Arc<dyn FileSource>>,
}

impl DatabaseFileSourceThread {
    fn new(
        online_file_source: Option<Arc<dyn FileSource>>,
        cache_path: &str,
        tile_server_options: &TileServerOptions,
    ) -> Self {
        let tile_server_options = match &online_file_source {
            Some(online) => online.get_resource_options().tile_server_options(),
            None => tile_server_options.clone(),
        };
        DatabaseFileSourceThread {
            db: RefCell::new(OfflineDatabase::new(cache_path, &tile_server_options)),
            online_file_source,
        }
    }

    /// `request(const Resource&, const ActorRef<FileSourceRequest>&)`.
    pub fn request(&self, resource: &Resource, req: &ActorRef<FileSourceRequestState>) {
        let mut offline_response =
            if resource.storage_policy != StoragePolicy::Volatile { self.db.borrow_mut().get(resource) } else { None };
        match &mut offline_response {
            None => {
                let mut response = Response::new();
                response.no_content = true;
                response.error = Some(Box::new(Error::new(Reason::NotFound, "Not found in offline database")));
                offline_response = Some(response);
            }
            Some(response) => {
                if !response.is_usable() {
                    response.error = Some(Box::new(Error::new(Reason::NotFound, "Cached resource is unusable")));
                }
            }
        }
        let response = offline_response.expect("response");
        req.invoke(move |request: &FileSourceRequestState| request.set_response(response));
    }

    /// `setDatabasePath(path, callback)`.
    pub fn set_database_path(&self, path: &str, callback: Option<Box<dyn FnOnce() + Send>>) {
        self.db.borrow_mut().change_path(path);
        if let Some(callback) = callback {
            callback();
        }
    }

    /// `forward(resource, response, callback)`.
    pub fn forward(&self, resource: &Resource, response: &Response, callback: Option<Box<dyn FnMut() + Send>>) {
        self.db.borrow_mut().put(resource, response);
        if let Some(mut callback) = callback {
            callback();
        }
    }

    /// `runPackDatabaseAutomatically(bool)`.
    pub fn run_pack_database_automatically(&self, autopack: bool) {
        self.db.borrow_mut().run_pack_database_automatically(autopack);
    }

    /// `put(resource, response)`.
    pub fn put(&self, resource: &Resource, response: &Response) {
        self.db.borrow_mut().put(resource, response);
    }

    /// `setOfflineMapboxTileCountLimit(uint64_t)`.
    pub fn set_offline_mapbox_tile_count_limit(&self, limit: u64) {
        self.db.borrow_mut().set_offline_mapbox_tile_count_limit(limit);
    }

    /// `reopenDatabaseReadOnly(bool)`.
    pub fn reopen_database_read_only(&self, read_only: bool) {
        self.db.borrow_mut().reopen_database_read_only(read_only);
    }
}

/// `DatabaseFileSource::Impl`.
struct Impl {
    thread: Thread<DatabaseFileSourceThread>,
    resource_options: Mutex<ResourceOptions>,
    client_options: Mutex<ClientOptions>,
}

impl Impl {
    fn actor(&self) -> ActorRef<DatabaseFileSourceThread> {
        self.thread.actor()
    }
}

/// `DatabaseFileSource`.
pub struct DatabaseFileSource {
    impl_: Impl,
}

impl DatabaseFileSource {
    /// `DatabaseFileSource(const ResourceOptions&, const ClientOptions&)`.
    pub fn new(resource_options: &ResourceOptions, client_options: &ClientOptions) -> Self {
        let online_file_source =
            FileSourceManager::get().get_file_source(FileSourceType::Network, resource_options, client_options);
        let cache_path = resource_options.cache_path().to_owned();
        let tile_server_options = resource_options.tile_server_options();
        // TODO(port): makeThreadPrioritySetter(EXPERIMENTAL_THREAD_PRIORITY_DATABASE).
        let thread = Thread::new("DatabaseFileSource", move |_| {
            DatabaseFileSourceThread::new(online_file_source, &cache_path, &tile_server_options)
        });
        DatabaseFileSource {
            impl_: Impl {
                thread,
                resource_options: Mutex::new(resource_options.clone()),
                client_options: Mutex::new(client_options.clone()),
            },
        }
    }

    /// `setDatabasePath(path, callback)`. The callback runs on the database
    /// thread, as in C++.
    pub fn set_database_path(&self, path: &str, callback: Option<Box<dyn FnOnce() + Send>>) {
        let path = path.to_owned();
        self.impl_.actor().invoke(move |t: &DatabaseFileSourceThread| t.set_database_path(&path, callback));
    }

    /// `runPackDatabaseAutomatically(bool)`.
    pub fn run_pack_database_automatically(&self, autopack: bool) {
        self.impl_.actor().invoke(move |t: &DatabaseFileSourceThread| t.run_pack_database_automatically(autopack));
    }

    /// `put(resource, response)`.
    pub fn put(&self, resource: &Resource, response: &Response) {
        let resource = resource.clone();
        let response = response.clone();
        self.impl_.actor().invoke(move |t: &DatabaseFileSourceThread| t.put(&resource, &response));
    }

    /// `setOfflineMapboxTileCountLimit(uint64_t)`.
    pub fn set_offline_mapbox_tile_count_limit(&self, limit: u64) {
        self.impl_.actor().invoke(move |t: &DatabaseFileSourceThread| t.set_offline_mapbox_tile_count_limit(limit));
    }
}

impl FileSource for DatabaseFileSource {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let req = FileSourceRequest::new(callback);
        let req_ref = req.actor();
        let resource = resource.clone();
        self.impl_.actor().invoke(move |t: &DatabaseFileSourceThread| t.request(&resource, &req_ref));
        Box::new(req)
    }

    fn forward(&self, res: &Resource, response: &Response, callback: Option<Box<dyn FnOnce() + Send>>) {
        if res.storage_policy == StoragePolicy::Volatile {
            return;
        }

        let wrapper: Option<Box<dyn FnMut() + Send>> =
            callback.map(|callback| Box::new(scheduler::bind_once(&scheduler::get_current(), callback)) as _);
        let res = res.clone();
        let response = response.clone();
        self.impl_.actor().invoke(move |t: &DatabaseFileSourceThread| t.forward(&res, &response, wrapper));
    }

    fn can_request(&self, resource: &Resource) -> bool {
        resource.has_loading_method(LoadingMethod::Cache)
            && !resource.url.starts_with(ASSET_PROTOCOL)
            && !resource.url.starts_with(FILE_PROTOCOL)
    }

    fn set_property(&self, key: &str, value: &Value) {
        match value {
            Value::Bool(read_only) if key == READ_ONLY_MODE_KEY => {
                let read_only = *read_only;
                self.impl_.actor().invoke(move |t: &DatabaseFileSourceThread| t.reopen_database_read_only(read_only));
            }
            _ => {
                let message = format!("Resource provider does not support property {key}");
                Log::error(Event::General, message);
            }
        }
    }

    fn pause(&self) {
        self.impl_.thread.pause();
    }

    fn resume(&self) {
        self.impl_.thread.resume();
    }

    fn set_resource_options(&self, options: ResourceOptions) {
        *self.impl_.resource_options.lock_unpoisoned() = options;
    }

    fn get_resource_options(&self) -> ResourceOptions {
        self.impl_.resource_options.lock_unpoisoned().clone()
    }

    fn set_client_options(&self, options: ClientOptions) {
        *self.impl_.client_options.lock_unpoisoned() = options;
    }

    fn get_client_options(&self) -> ClientOptions {
        self.impl_.client_options.lock_unpoisoned().clone()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}
