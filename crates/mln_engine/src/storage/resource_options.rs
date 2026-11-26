/// Where a [`Loader`](crate::storage::Loader) keeps its cache and finds its assets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceOptions {
    /// The path of the database that caches resources. The default,
    /// `":memory:"`, keeps the cache in memory.
    pub cache_path: String,
    /// The directory that `asset://` URLs load from.
    pub asset_path: String,
}

impl Default for ResourceOptions {
    fn default() -> Self {
        ResourceOptions { cache_path: ":memory:".to_owned(), asset_path: ".".to_owned() }
    }
}
