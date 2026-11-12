// Ports render-test/manifest_parser.{hpp,cpp}

use std::collections::BTreeSet;

use regex::Regex;

use crate::filesystem;
use crate::log::{self, Event};
use crate::metadata::TestPaths;
use crate::parser::read_json;
use crate::std_random::{Mt19937, SeedSeq, shuffle};

/// `Manifest`.
#[derive(Clone, Debug, Default)]
pub struct Manifest {
    manifest_path: String,
    test_root_path: String,
    result_path: String,
    cache_path: String,
    api_key: String,
    ignores: Vec<(String, String)>,
    test_paths: Vec<TestPaths>,
    /// `std::set<std::string>`: iterated in sorted order.
    probes: BTreeSet<String>,
}

impl Manifest {
    fn new() -> Self {
        Manifest { api_key: std::env::var("MLN_API_KEY").unwrap_or_default(), ..Default::default() }
    }

    #[must_use]
    pub fn get_ignores(&self) -> &[(String, String)] {
        &self.ignores
    }
    #[must_use]
    pub fn get_test_paths(&self) -> &[TestPaths] {
        &self.test_paths
    }
    #[must_use]
    pub fn get_test_root_path(&self) -> &str {
        &self.test_root_path
    }
    #[must_use]
    pub fn get_manifest_path(&self) -> &str {
        &self.manifest_path
    }
    #[must_use]
    pub fn get_result_path(&self) -> &str {
        &self.result_path
    }
    #[must_use]
    pub fn get_cache_path(&self) -> &str {
        &self.cache_path
    }
    #[must_use]
    pub fn get_api_key(&self) -> &str {
        &self.api_key
    }
    #[must_use]
    pub fn get_probes(&self) -> &BTreeSet<String> {
        &self.probes
    }

    /// `doShuffle(seed)`: the libstdc++ order (see `std_random.rs`).
    pub fn do_shuffle(&mut self, seed: u32) {
        let sequence = SeedSeq::new(&[seed]);
        let mut shuffler = Mt19937::from_seed_seq(&sequence);
        shuffle(&mut self.test_paths, &mut shuffler);
    }
}

fn parse_ignores(ignores_paths: &[String]) -> Vec<(String, String)> {
    let mut ignores = Vec::new();
    for path in ignores_paths {
        let Ok(members) = crate::parser::read_json_object_members(path) else {
            continue;
        };
        for (ignore, reason) in members {
            // C++ reads the value with GetString().
            let reason = reason.as_str().unwrap_or_default().to_string();
            ignores.push((ignore, reason));
        }
    }
    ignores
}

/// `getTestExpectations`.
///
/// `default_expectation_path`: absolute path that contains the style.json
/// file for testing; `test_id`: test case id that used for composing
/// expectation path; `expectated_paths`: absolute paths that contain possible
/// expected.png/metrics.json files for result checking.
fn get_test_expectations(default_expectation_path: &str, test_id: &str, expectated_paths: &[String]) -> Vec<String> {
    let mut expectations = vec![default_expectation_path.to_string()];
    for expected_path in expectated_paths {
        expectations.push(filesystem::join(expected_path, test_id));
    }
    expectations
}

/// `getValidPath(manifestPath, path)`.
///
/// C++ keeps the base path in a function-local `static`, so every manifest
/// after the first one in a process resolves against the first manifest's
/// directory. The runner parses one manifest per process; the port uses the
/// manifest it is given (unit tests parse several).
fn get_valid_path(manifest_path: &str, path: &str) -> String {
    let mut result = path.to_string();
    if filesystem::is_relative(&result) {
        result = filesystem::join(manifest_path, &result);
    }
    if filesystem::exists(&result) {
        return filesystem::lexically_normal(&result);
    }
    log::warning(Event::General, &format!("Invalid path is provided inside the manifest file: {path}"));
    String::new()
}

pub struct ManifestParser;

impl ManifestParser {
    /// `ManifestParser::parseManifest(manifestPath, testFilter)`.
    pub fn parse_manifest(manifest_path: &str, mut test_filter: String) -> Option<Manifest> {
        let mut manifest = Manifest::new();
        let file_path = manifest_path;
        // C++: manifestPath.substr(0, manifestPath.find(filename)).
        let name = filesystem::filename(file_path);
        manifest.manifest_path = match manifest_path.find(name) {
            Some(i) if !name.is_empty() => manifest_path[..i].to_string(),
            _ => manifest_path.to_string(),
        };

        let Ok(document) = read_json(file_path) else {
            log::error(Event::General, &format!("Provided manifest file: {file_path} is not a valid json"));
            return None;
        };

        let invalid = |what: &str| {
            log::warning(Event::General, &format!("Invalid {what} is provided inside the manifest file: {file_path}"));
        };

        if let Some(result_path_value) = document.get("result_path") {
            let Some(result_path) = result_path_value.as_str() else {
                invalid("result_path");
                return None;
            };
            manifest.result_path = filesystem::join(&get_valid_path(&manifest.manifest_path, result_path), "");
            if manifest.result_path.is_empty() {
                return None;
            }
        }
        if let Some(cache_path_value) = document.get("cache_path") {
            let Some(cache_path) = cache_path_value.as_str() else {
                invalid("cache_path");
                return None;
            };
            manifest.cache_path = filesystem::join(&get_valid_path(&manifest.manifest_path, "."), cache_path);
            if manifest.cache_path.is_empty() {
                return None;
            }
        }
        // TODO:PP
        if let Some(api_key_value) = document.get("access_token") {
            let Some(api_key) = api_key_value.as_str() else {
                invalid("access_token");
                return None;
            };
            manifest.api_key = api_key.to_string();
            if manifest.api_key.is_empty() {
                return None;
            }
        }
        let mut base_test_path = String::new();
        if let Some(test_path_value) = document.get("base_test_path") {
            let Some(test_path) = test_path_value.as_str() else {
                invalid("base_test_path");
                return None;
            };
            base_test_path = get_valid_path(&manifest.manifest_path, test_path);
            if base_test_path.is_empty() {
                return None;
            }
        }
        let mut expected_metric_path = String::new();
        if let Some(metric_path_value) = document.get("metric_path") {
            let Some(metric_path) = metric_path_value.as_str() else {
                invalid("metric_path");
                return None;
            };
            expected_metric_path = get_valid_path(&manifest.manifest_path, metric_path);
            if expected_metric_path.is_empty() {
                return None;
            }
        }
        let mut expectation_paths: Vec<String> = Vec::new();
        if let Some(expectation_path_value) = document.get("expectation_paths") {
            let Some(values) = expectation_path_value.as_array() else {
                log::warning(
                    Event::General,
                    &format!("Provided expectation_paths inside the manifest file: %s is not a valid array{file_path}"),
                );
                return None;
            };
            for value in values {
                let Some(value) = value.as_str() else {
                    invalid("expectation path item");
                    return None;
                };
                expectation_paths.push(get_valid_path(&manifest.manifest_path, value));
                if expectation_paths.last().is_some_and(String::is_empty) {
                    return None;
                }
            }
        }
        let mut ignore_paths: Vec<String> = Vec::new();
        if let Some(ignore_path_value) = document.get("ignore_paths") {
            let Some(values) = ignore_path_value.as_array() else {
                log::warning(
                    Event::General,
                    &format!("Provided ignore_paths inside the manifest file: {file_path} is not a valid array"),
                );
                return None;
            };
            for value in values {
                let Some(value) = value.as_str() else {
                    invalid("ignore path item");
                    return None;
                };
                ignore_paths.push(get_valid_path(&manifest.manifest_path, value));
                if ignore_paths.last().is_some_and(String::is_empty) {
                    return None;
                }
            }
            manifest.ignores = parse_ignores(&ignore_paths);
        }

        if let Some(probes_value) = document.get("probes") {
            let Some(values) = probes_value.as_array() else {
                log::warning(
                    Event::General,
                    &format!("Provided probes inside the manifest file: {file_path}is not a valid array"),
                );
                return None;
            };
            for value in values {
                let Some(value) = value.as_str() else {
                    invalid("probe type");
                    return None;
                };
                manifest.probes.insert(value.to_string());
            }
        }

        if test_filter.is_empty()
            && let Some(filter_value) = document.get("filter")
        {
            let Some(filter) = filter_value.as_str() else {
                invalid("filter");
                return None;
            };
            test_filter = filter.to_string();
        }

        manifest.test_root_path = base_test_path;
        if manifest.test_root_path.ends_with('/') {
            manifest.test_root_path.pop();
        }
        if manifest.manifest_path.ends_with('/') {
            manifest.manifest_path.pop();
        }
        if manifest.result_path.is_empty() {
            manifest.result_path = manifest.manifest_path.clone();
        } else if manifest.result_path.ends_with('/') {
            manifest.result_path.pop();
        }

        // C++ would iterate "" (and throw) without a base_test_path.
        if manifest.test_root_path.is_empty() {
            log::error(Event::General, &format!("No base_test_path is provided inside the manifest file: {file_path}"));
            return None;
        }

        // `std::regex(testFilter)` (ECMAScript) with `std::regex_search`; the
        // Rust regex dialect covers the filters in use (no look-around or
        // back-references).
        let filter = if test_filter.is_empty() {
            None
        } else {
            match Regex::new(&test_filter) {
                Ok(regex) => Some(regex),
                Err(e) => {
                    log::error(Event::General, &format!("Invalid test filter '{test_filter}': {e}"));
                    return None;
                }
            }
        };

        let path = manifest.test_root_path.clone();
        let entries = match filesystem::recursive_directory_entries(&path) {
            Ok(entries) => entries,
            Err(e) => {
                log::error(Event::General, &format!("Cannot list {path}: {e}"));
                return None;
            }
        };
        let root_length = manifest.test_root_path.len();
        for test_path in entries {
            // Skip paths that fail regexp search.
            if let Some(filter) = &filter
                && !filter.is_match(&test_path)
            {
                continue;
            }

            if filesystem::filename(&test_path) == "style.json" {
                let default_expectation_path = filesystem::remove_filename(&test_path);
                let test_id = default_expectation_path[root_length + 1..].to_string();

                let expected_metric_paths = vec![expected_metric_path.clone()];
                manifest.test_paths.push(TestPaths::new(
                    test_path,
                    get_test_expectations(&default_expectation_path, &test_id, &expectation_paths),
                    get_test_expectations(&default_expectation_path, &test_id, &expected_metric_paths),
                ));
            }
        }

        Some(manifest)
    }
}
