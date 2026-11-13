use mln_util::constants::DEFAULT_MAX_CACHE_SIZE;

/// Where file sources keep their cache and find their assets.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceOptions {
    /// The path of the database that caches resources. The default,
    /// `":memory:"`, keeps the cache in memory.
    pub cache_path: String,
    /// The root directory from where the `asset://` scheme gets resolved
    /// in a style.
    pub asset_path: String,
    /// The most bytes the cache database may hold.
    pub maximum_cache_size: u64,
}

impl Default for ResourceOptions {
    fn default() -> Self {
        ResourceOptions {
            cache_path: ":memory:".to_owned(),
            asset_path: ".".to_owned(),
            maximum_cache_size: DEFAULT_MAX_CACHE_SIZE,
        }
    }
}
