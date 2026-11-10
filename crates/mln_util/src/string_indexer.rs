// Ports include/mln/util/string_indexer.hpp and src/mln/util/string_indexer.cpp
// (gtests-core, for test/util/string_indexer.test.cpp; the port's gfx code
// keys uniforms and attributes by name, so nothing else uses it yet).
//
// C++ stores the strings in one char buffer and re-indexes its
// `string_view` map when the buffer reallocates; owned `String`s make that
// unnecessary. The shared mutex is an `RwLock`.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use crate::sync::RwLockExt;

pub type StringIdentity = usize;

#[derive(Default)]
struct Inner {
    string_to_identity: HashMap<String, StringIdentity>,
    identity_to_string: Vec<String>,
}

/// `StringIndexer`. C++ only constructs the singleton (`stringIndexer()`;
/// the constructor is protected and the test subclasses it); `new` is
/// public here for the same purpose.
#[derive(Default)]
pub struct StringIndexer {
    inner: RwLock<Inner>,
}

const INITIAL_CAPACITY: usize = 100;

impl StringIndexer {
    pub fn new() -> Self {
        StringIndexer {
            inner: RwLock::new(Inner {
                string_to_identity: HashMap::with_capacity(INITIAL_CAPACITY),
                identity_to_string: Vec::with_capacity(INITIAL_CAPACITY),
            }),
        }
    }

    /// `get(std::string_view)`: the identity of `string`, added if new.
    pub fn get(&self, string: &str) -> StringIdentity {
        {
            let inner = self.inner.read_unpoisoned();
            debug_assert!(inner.string_to_identity.len() == inner.identity_to_string.len());
            if let Some(&id) = inner.string_to_identity.get(string) {
                return id;
            }
        }

        let mut inner = self.inner.write_unpoisoned();
        debug_assert!(inner.string_to_identity.len() == inner.identity_to_string.len());
        if let Some(&id) = inner.string_to_identity.get(string) {
            // another writer inserted into the map
            return id;
        }
        // this writer to insert
        let id = inner.identity_to_string.len();
        inner.identity_to_string.push(string.to_owned());
        inner.string_to_identity.insert(string.to_owned(), id);
        id
    }

    /// `get(StringIdentity)`: asserts that `id` is known (debug builds), and
    /// returns an empty string for an unknown one.
    pub fn get_string(&self, id: StringIdentity) -> String {
        let inner = self.inner.read_unpoisoned();
        debug_assert!(id < inner.identity_to_string.len(), "id < identityToString.size()");
        inner.identity_to_string.get(id).cloned().unwrap_or_default()
    }

    /// `size()`.
    pub fn size(&self) -> usize {
        let inner = self.inner.read_unpoisoned();
        debug_assert!(inner.string_to_identity.len() == inner.identity_to_string.len());
        inner.identity_to_string.len()
    }
}

/// `stringIndexer()`: the singleton.
pub fn string_indexer() -> &'static StringIndexer {
    static INSTANCE: OnceLock<StringIndexer> = OnceLock::new();
    INSTANCE.get_or_init(StringIndexer::new)
}
