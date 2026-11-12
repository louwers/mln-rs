// Ports include/mln/storage/resource_options.hpp and
// src/mln/storage/resource_options.cpp.

use mln_util::constants::DEFAULT_MAX_CACHE_SIZE;

/// `ResourceOptions`. `Clone` is `clone()`.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceOptions {
    cache_path: String,
    asset_path: String,
    maximum_size: u64,
}

impl Default for ResourceOptions {
    /// `ResourceOptions()`: default values.
    fn default() -> Self {
        ResourceOptions {
            cache_path: ":memory:".to_owned(),
            asset_path: ".".to_owned(),
            maximum_size: DEFAULT_MAX_CACHE_SIZE,
        }
    }
}

impl ResourceOptions {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_cache_path(mut self, path: impl Into<String>) -> Self {
        self.cache_path = path.into();
        self
    }

    #[must_use]
    pub fn cache_path(&self) -> &str {
        &self.cache_path
    }

    /// Sets the asset path, which is the root directory from where the
    /// asset:// scheme gets resolved in a style.
    #[must_use]
    pub fn with_asset_path(mut self, path: impl Into<String>) -> Self {
        self.asset_path = path.into();
        self
    }

    #[must_use]
    pub fn asset_path(&self) -> &str {
        &self.asset_path
    }

    #[must_use]
    pub fn with_maximum_cache_size(mut self, size: u64) -> Self {
        self.maximum_size = size;
        self
    }

    #[must_use]
    pub fn maximum_cache_size(&self) -> u64 {
        self.maximum_size
    }
}
