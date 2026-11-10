// Ports include/mln/storage/file_source_manager.hpp,
// src/mln/storage/file_source_manager.cpp and
// platform/default/src/mln/storage/file_source_manager.cpp (the default
// factories).
//
// C++ holds a recursive mutex while a factory runs (the MainResourceLoader
// factory asks the manager for its own file sources). Rust has no recursive
// mutex in std, so the lock is released while the factory runs; on one
// thread the result is the same. If two threads create the same file source
// at once, the first one registered wins and the other is dropped.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use mln_util::client_options::ClientOptions;
use mln_util::string::to_string_u64;
use mln_util::sync::MutexExt;

use crate::asset_file_source::AssetFileSource;
#[cfg(feature = "sqlite")]
use crate::database_file_source::DatabaseFileSource;
use crate::file_source::{FileSource, FileSourceType};
use crate::local_file_source::LocalFileSource;
use crate::main_resource_loader::MainResourceLoader;
#[cfg(feature = "sqlite")]
use crate::mbtiles_file_source::MBTilesFileSource;
use crate::online_file_source::OnlineFileSource;
use crate::pmtiles_file_source::PMTilesFileSource;
use crate::resource_options::ResourceOptions;

/// `FileSourceManager::FileSourceFactory`.
pub type FileSourceFactory = Arc<dyn Fn(&ResourceOptions, &ClientOptions) -> Arc<dyn FileSource> + Send + Sync>;

struct FileSourceInfo {
    type_: FileSourceType,
    id: String,
    file_source: Weak<dyn FileSource>,
}

#[derive(Default)]
struct Impl {
    file_sources: Vec<FileSourceInfo>,
    file_source_factories: BTreeMap<FileSourceType, FileSourceFactory>,
}

/// `FileSourceManager`: a singleton (`get()`) that creates and shares file
/// sources per (type, configuration).
pub struct FileSourceManager {
    impl_: Mutex<Impl>,
}

impl FileSourceManager {
    fn new() -> Self {
        FileSourceManager { impl_: Mutex::new(Impl::default()) }
    }

    /// `FileSourceManager::get()`: the `DefaultFileSourceManagerImpl`.
    pub fn get() -> &'static FileSourceManager {
        static INSTANCE: OnceLock<FileSourceManager> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let manager = FileSourceManager::new();
            manager.register_file_source_factory(
                FileSourceType::ResourceLoader,
                Arc::new(|resource_options: &ResourceOptions, client_options: &ClientOptions| {
                    Arc::new(MainResourceLoader::new(resource_options, client_options)) as Arc<dyn FileSource>
                }),
            );
            manager.register_file_source_factory(
                FileSourceType::Asset,
                Arc::new(|resource_options: &ResourceOptions, client_options: &ClientOptions| {
                    Arc::new(AssetFileSource::new(resource_options, client_options)) as Arc<dyn FileSource>
                }),
            );
            #[cfg(feature = "sqlite")]
            manager.register_file_source_factory(
                FileSourceType::Database,
                Arc::new(|resource_options: &ResourceOptions, client_options: &ClientOptions| {
                    Arc::new(DatabaseFileSource::new(resource_options, client_options)) as Arc<dyn FileSource>
                }),
            );
            manager.register_file_source_factory(
                FileSourceType::FileSystem,
                Arc::new(|resource_options: &ResourceOptions, client_options: &ClientOptions| {
                    Arc::new(LocalFileSource::new(resource_options, client_options)) as Arc<dyn FileSource>
                }),
            );
            #[cfg(feature = "sqlite")]
            manager.register_file_source_factory(
                FileSourceType::Mbtiles,
                Arc::new(|resource_options: &ResourceOptions, client_options: &ClientOptions| {
                    Arc::new(MBTilesFileSource::new(resource_options, client_options)) as Arc<dyn FileSource>
                }),
            );
            manager.register_file_source_factory(
                FileSourceType::Pmtiles,
                Arc::new(|resource_options: &ResourceOptions, client_options: &ClientOptions| {
                    Arc::new(PMTilesFileSource::new(resource_options, client_options)) as Arc<dyn FileSource>
                }),
            );
            manager.register_file_source_factory(
                FileSourceType::Network,
                Arc::new(|resource_options: &ResourceOptions, client_options: &ClientOptions| {
                    Arc::new(OnlineFileSource::new(resource_options, client_options)) as Arc<dyn FileSource>
                }),
            );
            manager
        })
    }

    fn file_source_id(resource_options: &ResourceOptions) -> String {
        let context = resource_options.platform_context() as u64;
        let base_url = resource_options.tile_server_options().base_url().to_owned();
        base_url
            + "|"
            + resource_options.api_key()
            + "|"
            + resource_options.cache_path()
            + "|"
            + &to_string_u64(context)
    }

    /// Returns shared instance of a file source for (type, options) tuple.
    /// Creates new instance via registered factory if needed. If new instance
    /// cannot be created, `None` would be returned.
    pub fn get_file_source(
        &self,
        type_: FileSourceType,
        resource_options: &ResourceOptions,
        client_options: &ClientOptions,
    ) -> Option<Arc<dyn FileSource>> {
        let id = Self::file_source_id(resource_options);

        let factory = {
            let mut impl_ = self.impl_.lock_unpoisoned();

            // Remove released file sources.
            impl_.file_sources.retain(|info| info.file_source.strong_count() > 0);

            if let Some(file_source) = impl_
                .file_sources
                .iter()
                .find(|info| info.type_ == type_ && info.id == id)
                .and_then(|info| info.file_source.upgrade())
            {
                return Some(file_source);
            }

            impl_.file_source_factories.get(&type_)?.clone()
        };

        let file_source = factory(resource_options, client_options);

        let existing = {
            let mut impl_ = self.impl_.lock_unpoisoned();
            let existing = impl_
                .file_sources
                .iter()
                .find(|info| info.type_ == type_ && info.id == id)
                .and_then(|info| info.file_source.upgrade());
            if existing.is_none() {
                impl_.file_sources.push(FileSourceInfo { type_, id, file_source: Arc::downgrade(&file_source) });
            }
            existing
        };
        match existing {
            Some(existing) => Some(existing),
            None => Some(file_source),
        }
    }

    /// Registers file source factory for a provided FileSourceType type. If
    /// factory for the same type was already registered, will unregister
    /// previously registered factory.
    pub fn register_file_source_factory(&self, type_: FileSourceType, factory: FileSourceFactory) {
        self.impl_.lock_unpoisoned().file_source_factories.insert(type_, factory);
    }

    /// Unregisters file source factory. If there are no registered factories
    /// for a FileSourceType invocation has no effect.
    pub fn un_register_file_source_factory(&self, type_: FileSourceType) -> Option<FileSourceFactory> {
        self.impl_.lock_unpoisoned().file_source_factories.remove(&type_)
    }
}
