//! Downloads, through curl: at most [`PARALLEL`] requests at once, each
//! response kept in a download directory so that building the snapshot again
//! fetches only what it lacks.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The most requests in flight at once, to be polite to the servers.
const PARALLEL: usize = 8;

/// The User-Agent of every request: the tool, and nothing about who runs it.
const USER_AGENT: &str = "mln-rs-bench-fixtures";

/// One file to download.
pub(crate) struct Download {
    pub(crate) url: String,
    /// Where the response goes.
    pub(crate) path: PathBuf,
}

/// What a download found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// The response is at the download's path; it may be empty.
    Body,
    /// The server has nothing at the URL (404).
    Missing,
}

/// Fetches every download whose path doesn't exist yet, and says what each
/// found. A 204 response gives an empty file, and a 404 is [`Found::Missing`],
/// remembered with a `.missing` file next to the path. Any other response or
/// a failed transfer fails the whole fetch.
pub(crate) fn fetch(downloads: &[&Download]) -> Result<Vec<Found>, String> {
    let pending: Vec<&Download> = downloads
        .iter()
        .copied()
        .filter(|download| !download.path.exists() && !missing_marker(download).exists())
        .collect();
    if !pending.is_empty() {
        eprintln!("mln-bench-snapshot: downloading {} files", pending.len());
        run_curl(&pending)?;
    }
    Ok(downloads.iter().map(|download| if download.path.exists() { Found::Body } else { Found::Missing }).collect())
}

fn missing_marker(download: &Download) -> PathBuf {
    let mut name = download.path.file_name().expect("a download's file name").to_owned();
    name.push(".missing");
    download.path.with_file_name(name)
}

fn partial(path: &Path) -> PathBuf {
    let mut name = path.file_name().expect("a download's file name").to_owned();
    name.push(".part");
    path.with_file_name(name)
}

fn run_curl(pending: &[&Download]) -> Result<(), String> {
    let mut config = String::new();
    for download in pending {
        let directory = download.path.parent().expect("a download's directory");
        std::fs::create_dir_all(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
        let output = partial(&download.path);
        writeln!(config, "url = \"{}\"\noutput = \"{}\"", quote(&download.url), quote(&output.to_string_lossy()))
            .expect("writing to a string");
    }
    let config_file = tempfile::NamedTempFile::new().map_err(|error| format!("a temporary file: {error}"))?;
    std::fs::write(config_file.path(), config).map_err(|error| format!("curl's configuration: {error}"))?;
    let output = Command::new("curl")
        .args(["--parallel", "--parallel-max", &PARALLEL.to_string()])
        .args(["--silent", "--show-error", "--location", "--compressed", "--retry", "3"])
        .args(["--user-agent", USER_AGENT])
        .args(["--write-out", "%{response_code} %{url}\\n"])
        .arg("--config")
        .arg(config_file.path())
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("curl: {error}"))?;
    let codes: BTreeMap<&str, &str> = std::str::from_utf8(&output.stdout)
        .map_err(|_| "curl's output isn't UTF-8".to_owned())?
        .lines()
        .filter_map(|line| line.split_once(' ').map(|(code, url)| (url, code)))
        .collect();
    let mut failures = Vec::new();
    for download in pending {
        let output = partial(&download.path);
        match codes.get(download.url.as_str()).copied() {
            Some("200") => std::fs::rename(&output, &download.path)
                .map_err(|error| format!("{}: {error}", download.path.display()))?,
            Some("204") => {
                std::fs::write(&download.path, []).map_err(|error| format!("{}: {error}", download.path.display()))?;
            }
            Some("404") => {
                std::fs::write(missing_marker(download), [])
                    .map_err(|error| format!("{}: {error}", download.path.display()))?;
            }
            code => failures.push(format!("{} ({})", download.url, code.unwrap_or("no response"))),
        }
        // A response that wasn't kept, such as a 404's page.
        let _ = std::fs::remove_file(&output);
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("{} downloads failed:\n  {}", failures.len(), failures.join("\n  ")))
    }
}

/// `text` inside the double quotes of a curl configuration file.
fn quote(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `segment` as one segment of a URL's path.
pub(crate) fn encode_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            write!(encoded, "%{byte:02X}").expect("writing to a string");
        }
    }
    encoded
}
