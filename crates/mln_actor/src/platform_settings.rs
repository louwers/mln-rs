// Ports include/mln/platform/settings.hpp and src/mln/platform/settings.cpp
// (gtests-core, for test/platform/settings.test.cpp). It lives in mln_actor
// because its C++ users are the thread priority setters of util::Thread and
// the thread pools (still TODO(port) here).

use std::sync::{Mutex, OnceLock};

use feature::{Value, ValueObject};
use mln_util::sync::MutexExt;

// The value for EXPERIMENTAL_THREAD_PRIORITY_* keys, must be a double.
pub const EXPERIMENTAL_THREAD_PRIORITY_WORKER: &str = "maplibre_thread_priority_worker";
pub const EXPERIMENTAL_THREAD_PRIORITY_FILE: &str = "maplibre_thread_priority_file";
pub const EXPERIMENTAL_THREAD_PRIORITY_NETWORK: &str = "maplibre_thread_priority_network";
pub const EXPERIMENTAL_THREAD_PRIORITY_DATABASE: &str = "maplibre_thread_priority_database";

/// Settings class provides non-persistent, in-process key-value storage.
pub struct Settings {
    settings: Mutex<ValueObject>,
}

fn get_value(key: &str, object: &ValueObject) -> Value {
    object.get(key).cloned().unwrap_or_default()
}

impl Settings {
    /// Returns singleton instance
    pub fn get_instance() -> &'static Settings {
        static INSTANCE: OnceLock<Settings> = OnceLock::new();
        INSTANCE.get_or_init(|| Settings { settings: Mutex::new(ValueObject::new()) })
    }

    /// Sets setting value for a specified key.
    pub fn set(&self, key: &str, value: Value) {
        self.settings.lock_unpoisoned().insert(key.to_owned(), value);
    }

    /// Sets multiple setting values by merging current Settings object
    /// with provided `values` object.
    pub fn set_values(&self, values: &ValueObject) {
        let mut settings = self.settings.lock_unpoisoned();
        for (key, value) in values {
            settings.insert(key.clone(), value.clone());
        }
    }

    /// Returns setting value for a specified key or NullValue if element
    /// for specified key is missing.
    pub fn get(&self, key: &str) -> Value {
        get_value(key, &self.settings.lock_unpoisoned())
    }

    /// Returns values for settings whose keys are equal to the ones provided in
    /// `keys` argument. Null values would be provided for keys whose elements
    /// are missing in Settings.
    pub fn get_values(&self, keys: &[&str]) -> ValueObject {
        let settings = self.settings.lock_unpoisoned();
        keys.iter().map(|key| ((*key).to_owned(), get_value(key, &settings))).collect()
    }
}
