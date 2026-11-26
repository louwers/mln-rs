//! A network that only frames advance, under the real online file source:
//! a server that answers from the fixtures, links of a round-trip time and
//! a bandwidth that the transfers in flight share, and faults drawn from a
//! fixed seed, so that each transfer takes the same frames on every run.
//!
//! The online file source runs as an application's does, its limit on
//! transfers in flight, their priorities, cancellation, retries after
//! errors and revalidations included, through
//! `mln_storage::simulated_network`: its transfers come to [`Simulator`], and
//! its deadlines go by the network's clock, one 60 Hz frame per
//! [`Network::advance`].

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mln_storage::simulated_network::{Clock, OnlineDriver, Transfer, Transport, online_file_source};
use mln_storage::{Cache, FileSource, Loader, Resource};
use mln_style::hash::{HashMap, HashSet};
use mln_util::pool::Pool;
use parking_lot::Mutex;

use super::fixtures::Fixtures;

/// The interval of a frame, in nanoseconds: a 60 Hz display's.
pub(crate) const FRAME: u64 = 16_666_667;

/// Where the server's resources are.
const ORIGIN: &str = "https://bench.invalid/";

/// The bytes of a response's status line and headers, which every answer
/// takes on the wire before its body.
const HEADER_BYTES: u64 = 300;

/// The seed of the faults: which transfers fail.
const SEED: u64 = 0x6d6c_6e2d_6e65_7477;

/// A link between the map and the server, and the faults on it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Profile {
    /// From a transfer's start to the first byte of its answer.
    pub(crate) rtt: Duration,
    /// Bytes per second, which the transfers receiving share equally;
    /// `None` takes no time.
    pub(crate) bandwidth: Option<u64>,
    /// The share of transfers that fail with a connection error after a
    /// round trip, in thousandths.
    pub(crate) failures: u64,
    /// The share of transfers that the server answers with a 503, and the
    /// share it answers with a 429, in thousandths each. Neither says when
    /// to retry: a `Retry-After` in seconds counts from the system's clock,
    /// which the network's doesn't follow.
    pub(crate) server_errors: u64,
    /// A time, from [`Network::begin_faults`], from which until another
    /// every transfer fails: those in flight at its start fail at once, and
    /// those whose answer would come during it fail then.
    pub(crate) outage: Option<(Duration, Duration)>,
}

impl Profile {
    const fn link(rtt_ms: u64, bandwidth: Option<u64>) -> Profile {
        Profile { rtt: Duration::from_millis(rtt_ms), bandwidth, failures: 0, server_errors: 0, outage: None }
    }
}

/// Bytes per second of `mbit` megabits per second, in tenths.
const fn mbit(tenths: u64) -> u64 {
    tenths * 100_000 / 8
}

/// A server on the same machine: no time at all, so that each answer
/// comes the frame after its request.
pub(crate) const LOCAL: Profile = Profile::link(0, None);
/// A CDN's edge near the user, over a fast link.
pub(crate) const CDN: Profile = Profile::link(50, Some(mbit(500)));
/// A mobile 4G link, as browsers' throttling presets model one.
pub(crate) const MOBILE_4G: Profile = Profile::link(170, Some(mbit(90)));
/// A slow mobile link (browsers' "slow 4G" / "fast 3G").
pub(crate) const SLOW: Profile = Profile::link(300, Some(mbit(16)));
/// [`MOBILE_4G`] that drops: 2% of transfers fail, and for 3 seconds,
/// from a second after [`Network::begin_faults`], every transfer does.
pub(crate) const FLAKY: Profile =
    Profile { failures: 20, outage: Some((Duration::from_secs(1), Duration::from_secs(4))), ..MOBILE_4G };

/// What went over the network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Transfers started.
    pub(crate) requests: u64,
    /// Transfers that the map no longer wanted before their answer came.
    pub(crate) cancelled: u64,
    /// Bytes received, headers included.
    pub(crate) bytes: u64,
    /// Bytes received of transfers that were cancelled or failed, a 5xx
    /// or 429 answer's included.
    pub(crate) wasted: u64,
    /// Transfers of a resource (a URL, and a byte range of it) whose last
    /// transfer failed.
    pub(crate) retries: u64,
    /// Transfers of a resource that an earlier transfer brought.
    pub(crate) refetches: u64,
}

impl Counts {
    /// Declares the counts in `bench`, since `before`.
    pub(crate) fn declare(&self, bench: &mut crate::scenario::Bench, before: Counts) {
        bench.count("requests", self.requests - before.requests);
        bench.count("requests_cancelled", self.cancelled - before.cancelled);
        bench.count("retries", self.retries - before.retries);
        bench.count("refetches", self.refetches - before.refetches);
        bench.bytes("bytes_transferred", self.bytes - before.bytes);
        bench.bytes("bytes_wasted", self.wasted - before.wasted);
    }
}

/// What answers the network's transfers: the [`Server`], or a test's.
trait Origin: Send + Sync {
    fn answer(&self, resource: &Resource) -> Answer;
}

/// What the server answers.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    /// Decoded, as an HTTP stack hands it over.
    body: Vec<u8>,
    /// The bytes on the wire, headers included: an archive's tiles as it
    /// holds them, compressed.
    wire: u64,
}

/// How a server serves a tile set.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Serve {
    /// A TileJSON and a URL per tile, `z/x/y`.
    Tiles,
    /// The PMTiles archive, whose byte ranges the map reads
    /// (`pmtiles://https://...`).
    Archive,
}

/// A tile set the server serves, from an archive of the snapshot.
struct TileSet {
    /// The archive's path in the snapshot without its extension, such as
    /// `tiles/street`, which the set's URLs start with.
    name: String,
    archive_path: PathBuf,
    archive: pmtiles::PmTilesReader<std::fs::File, pmtiles::NoCache>,
    tilejson: String,
}

impl TileSet {
    /// The set of the snapshot's archive at `archive`, such as
    /// `tiles/street.pmtiles`.
    fn new(fixtures: &Fixtures, archive: &str) -> TileSet {
        let name = archive.strip_suffix(".pmtiles").expect("a PMTiles archive").to_owned();
        let archive_path = fixtures.path(archive);
        let archive = pmtiles::PmTilesReader::new_with_path(&archive_path)
            .unwrap_or_else(|error| panic!("{}: {error}", archive_path.display()));
        let header = archive.get_header();
        let tilejson = serde_json::json!({
            "tilejson": "3.0.0",
            "scheme": "xyz",
            "tiles": [format!("{ORIGIN}{name}/{{z}}/{{x}}/{{y}}")],
            "minzoom": header.min_zoom,
            "maxzoom": header.max_zoom,
            "bounds": [header.min_longitude, header.min_latitude, header.max_longitude, header.max_latitude],
        })
        .to_string();
        TileSet { name, archive_path, archive, tilejson }
    }

    /// The source URL of the set, served as `serve` says.
    fn url(&self, serve: Serve) -> String {
        match serve {
            Serve::Tiles => format!("{ORIGIN}{}.json", self.name),
            Serve::Archive => format!("pmtiles://{ORIGIN}{}.pmtiles", self.name),
        }
    }

    /// The bytes `first` to `last` of the archive, as many as it has.
    fn read_range(&self, (first, last): (u64, u64)) -> Vec<u8> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(&self.archive_path).expect("the archive opens");
        file.seek(SeekFrom::Start(first)).expect("the archive seeks");
        let mut bytes = Vec::new();
        file.take(last + 1 - first).read_to_end(&mut bytes).expect("the archive reads");
        bytes
    }

    /// The set's TileJSON or tile at `path`, and its size on the wire.
    fn read(&self, path: &str) -> Option<(Vec<u8>, u64)> {
        let path = path.strip_prefix(&self.name)?;
        if path == ".json" {
            return Some((self.tilejson.clone().into_bytes(), self.tilejson.len() as u64));
        }
        let zxy = path.strip_prefix('/')?;
        let mut parts = zxy.split('/').map(|part| part.parse::<u32>().expect("a tile's z/x/y"));
        let (z, x, y) = (parts.next()?, parts.next()?, parts.next()?);
        let coord = pmtiles::TileCoord::new(u8::try_from(z).ok()?, x, y).expect("a tile in the world");
        let raw = self.archive.get_tile(coord).expect("the archive reads")?;
        let body = self.archive.get_tile_decompressed(coord).expect("the archive reads")?;
        Some((body.to_vec(), raw.len() as u64))
    }
}

/// A server of a style over the fixtures, at [`ORIGIN`]: the style, the
/// tile sets of its sources as [`Serve`] says, and the snapshot's sprite
/// and glyphs.
/// Every answer carries an `ETag`, which a revalidation that sends it back
/// gets a 304 for, and a `Cache-Control: max-age` of [`Server::MAX_AGE`],
/// or an `Expires` long past after [`set_expired`](Self::set_expired).
/// Either is a day or more from the system's clock, which times the
/// expiries, so that whether one has passed never depends on when a run
/// started.
pub(crate) struct Server {
    dir: PathBuf,
    style: String,
    sets: Vec<TileSet>,
    expired: AtomicBool,
}

impl Server {
    /// `style`, a style over the snapshot such as
    /// [`Snapshot::style`](mln_bench_workload::Snapshot::style), with every
    /// URL the server's: each source of an archive of the snapshot
    /// (`pmtiles://asset://...`) served as `serve` says.
    pub(crate) fn new(fixtures: &Fixtures, style: &str, serve: Serve) -> Arc<Server> {
        let mut style: serde_json::Value = serde_json::from_str(style).expect("the style is JSON");
        let mut sets = Vec::new();
        for source in style["sources"].as_object_mut().expect("a style's sources").values_mut() {
            let Some(archive) = source["url"].as_str().and_then(|url| url.strip_prefix("pmtiles://asset://")) else {
                continue;
            };
            let set = TileSet::new(fixtures, archive);
            source["url"] = set.url(serve).into();
            sets.push(set);
        }
        for url in ["sprite", "glyphs"] {
            let local = style[url].as_str().expect("the style's sprite and glyphs");
            style[url] = local.replacen("asset://", ORIGIN, 1).into();
        }
        Arc::new(Server {
            dir: fixtures.dir().to_owned(),
            style: style.to_string(),
            sets,
            expired: AtomicBool::new(false),
        })
    }

    /// How long a client may use what the server answers, in seconds: a
    /// week, longer than any scenario runs.
    pub(crate) const MAX_AGE: u64 = 7 * 24 * 3600;

    /// The style's URL.
    pub(crate) fn style_url() -> String {
        format!("{ORIGIN}style.json")
    }

    /// Whether what the server answers from now on has expired already.
    pub(crate) fn set_expired(&self, expired: bool) {
        self.expired.store(expired, Ordering::Relaxed);
    }

    /// The body at `path`, and its size on the wire.
    fn read(&self, path: &str) -> Option<(Vec<u8>, u64)> {
        if path == "style.json" {
            return Some((self.style.clone().into_bytes(), self.style.len() as u64));
        }
        if let Some(found) = self.sets.iter().find_map(|set| set.read(path)) {
            return Some(found);
        }
        let decoded = percent_encoding::percent_decode_str(path).decode_utf8().expect("a UTF-8 path");
        let body = std::fs::read(self.dir.join(decoded.as_ref())).ok()?;
        let wire = body.len() as u64;
        Some((body, wire))
    }
}

impl Origin for Server {
    fn answer(&self, resource: &Resource) -> Answer {
        let url = &resource.url;
        let etag = format!("\"{:016x}\"", fnv(&[url.as_bytes()]));
        let expiry = if self.expired.load(Ordering::Relaxed) {
            // The same date in every answer: a server that answers a
            // revalidation with a later one than before, as one counted
            // from the system's clock would once a second had passed, has
            // the online file source take its clock for skewed and the data
            // for fresh.
            ("Expires".to_owned(), "Wed, 01 Jan 2025 00:00:00 GMT".to_owned())
        } else {
            ("Cache-Control".to_owned(), format!("max-age={}", Server::MAX_AGE))
        };
        let mut headers = vec![expiry, ("ETag".to_owned(), etag.clone())];
        if resource.prior_etag.as_deref() == Some(etag.as_str()) {
            return Answer { status: 304, headers, body: Vec::new(), wire: HEADER_BYTES };
        }
        let path = url.strip_prefix(ORIGIN).unwrap_or_else(|| panic!("a request off the server: {url}"));
        let archive = path.strip_suffix(".pmtiles");
        if let Some(set) = self.sets.iter().find(|set| archive == Some(set.name.as_str())) {
            let range = resource.data_range.expect("a range of the archive");
            let body = set.read_range(range);
            let wire = HEADER_BYTES + body.len() as u64;
            return Answer { status: 206, headers, body, wire };
        }
        let found = self.read(path);
        let Some((body, wire)) = found else {
            headers.clear();
            return Answer { status: 404, headers, body: Vec::new(), wire: HEADER_BYTES };
        };
        Answer { status: 200, headers, body, wire: HEADER_BYTES + wire }
    }
}

/// What names a resource among transfers: its URL, and the byte range of it,
/// as a PMTiles archive's header, directories and tiles are ranges of one URL.
fn key(resource: &Resource) -> String {
    match resource.data_range {
        Some((first, last)) => format!("{} {first}-{last}", resource.url),
        None => resource.url.clone(),
    }
}

/// FNV-1a over `parts`.
fn fnv(parts: &[&[u8]]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for part in parts {
        for &byte in *part {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
        // Parts that run together differently hash differently.
        hash = (hash ^ 0xff).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Nanobytes: bytes times 10^9, so that a bandwidth in bytes per second
/// over nanoseconds moves whole units.
const NANO: u128 = 1_000_000_000;

/// A transfer the network carries.
struct InFlight {
    /// In the order the transfers started.
    seq: u64,
    transfer: Transfer,
    /// When its answer's first byte comes, in nanoseconds of the network's
    /// clock.
    first_byte: u64,
    /// `None` fails it with a connection error then.
    answer: Option<Answer>,
    /// Whether its first byte has come.
    receiving: bool,
    /// Its wire bytes, and those still to come, in nanobytes.
    size: u128,
    left: u128,
}

impl InFlight {
    fn received(&self) -> u64 {
        ((self.size - self.left) / NANO) as u64
    }
}

/// How a transfer ended, at a time of the network's clock.
enum Ended {
    Answered(Answer),
    Failed,
}

struct State {
    /// The network's clock, in nanoseconds.
    now: u64,
    profile: Profile,
    /// When the faults' clock started ([`Network::begin_faults`]); no fault
    /// happens before.
    faults_from: Option<u64>,
    /// Transfers started since the last advance, at `now`.
    started: Vec<Transfer>,
    seq: u64,
    flight: Vec<InFlight>,
    /// Transfers that ended, for [`Network::pump`], with when and in which
    /// order.
    ended: Vec<(u64, u64, Transfer, Ended)>,
    /// Transfers per resource ([`key`]), and the resources whose last
    /// transfer failed or brought data.
    attempts: HashMap<String, u64>,
    failed: HashSet<String>,
    brought: HashSet<String>,
    counts: Counts,
}

/// The transport of the online file source: it takes each transfer, and
/// [`Network::advance`] carries it.
struct Simulator {
    origin: Arc<dyn Origin>,
    state: Mutex<State>,
}

impl Transport for Simulator {
    fn start(&self, transfer: Transfer) {
        // What the transfer is, its answer and its faults are found when
        // the network advances, outside the frame's work.
        self.state.lock().started.push(transfer);
    }
}

impl State {
    fn new(profile: Profile) -> State {
        State {
            now: 0,
            profile,
            faults_from: None,
            started: Vec::new(),
            seq: 0,
            flight: Vec::new(),
            ended: Vec::new(),
            attempts: HashMap::default(),
            failed: HashSet::default(),
            brought: HashSet::default(),
            counts: Counts::default(),
        }
    }

    fn outage(&self) -> Option<(u64, u64)> {
        let (from, until) = self.profile.outage?;
        let origin = self.faults_from?;
        Some((origin + from.as_nanos() as u64, origin + until.as_nanos() as u64))
    }

    fn in_outage(&self, at: u64) -> bool {
        self.outage().is_some_and(|(from, until)| (from..until).contains(&at))
    }

    /// Whether the `attempt`th transfer of resource `url` ([`key`]) draws a
    /// fault of `rate`
    /// thousandths, of the kind `salt` names. The draw depends on the
    /// transfer alone, so that a change in what else the map requests, or
    /// in what order, doesn't move the faults.
    fn draws(&self, url: &str, attempt: u64, salt: u8, rate: u64) -> bool {
        self.faults_from.is_some()
            && fnv(&[&SEED.to_le_bytes(), url.as_bytes(), &attempt.to_le_bytes(), &[salt]]) % 1000 < rate
    }

    /// Ends `flight` at `at`.
    fn end(&mut self, at: u64, flight: InFlight, ended: Ended) {
        let url = key(flight.transfer.resource());
        match &ended {
            Ended::Answered(answer) => {
                self.counts.bytes += answer.wire;
                if answer.status >= 500 || answer.status == 429 {
                    self.counts.wasted += answer.wire;
                    self.failed.insert(url);
                } else {
                    self.failed.remove(&url);
                    if matches!(answer.status, 200 | 206) {
                        self.brought.insert(url);
                    }
                }
            }
            Ended::Failed => {
                let received = flight.received();
                self.counts.bytes += received;
                self.counts.wasted += received;
                self.failed.insert(url);
            }
        }
        self.ended.push((at, flight.seq, flight.transfer, ended));
    }

    /// Takes in the transfers started since the last advance.
    fn take_started(&mut self, origin: &dyn Origin) {
        let profile = self.profile;
        let rtt = profile.rtt.as_nanos() as u64;
        for transfer in std::mem::take(&mut self.started) {
            self.counts.requests += 1;
            if transfer.is_cancelled() {
                self.counts.cancelled += 1;
                continue;
            }
            let url = key(transfer.resource());
            let attempt = self.attempts.entry(url.clone()).or_default();
            *attempt += 1;
            let attempt = *attempt;
            if self.failed.contains(&url) {
                self.counts.retries += 1;
            }
            if self.brought.contains(&url) {
                self.counts.refetches += 1;
            }
            let answer = if self.draws(&url, attempt, 0, profile.failures) {
                None
            } else if self.draws(&url, attempt, 1, profile.server_errors) {
                Some(Answer { status: 503, headers: vec![], body: Vec::new(), wire: HEADER_BYTES })
            } else if self.draws(&url, attempt, 2, profile.server_errors) {
                Some(Answer { status: 429, headers: vec![], body: Vec::new(), wire: HEADER_BYTES })
            } else {
                Some(origin.answer(transfer.resource()))
            };
            let size = u128::from(answer.as_ref().map_or(0, |answer| answer.wire)) * NANO;
            self.seq += 1;
            let seq = self.seq;
            self.flight.push(InFlight {
                seq,
                transfer,
                first_byte: self.now + rtt,
                answer,
                receiving: false,
                size,
                left: size,
            });
        }
    }

    /// Ends what ends at `at`: every transfer at an outage's start, a
    /// transfer whose first byte comes during an outage or that drew a
    /// failure, and a transfer whose last byte came.
    fn events_at(&mut self, at: u64) {
        if self.outage().is_some_and(|(from, _)| from == at) {
            for flight in std::mem::take(&mut self.flight) {
                self.end(at, flight, Ended::Failed);
            }
        }
        let unlimited = self.profile.bandwidth.is_none();
        let down = self.in_outage(at);
        let mut index = 0;
        while index < self.flight.len() {
            let flight = &mut self.flight[index];
            if !flight.receiving && flight.first_byte == at {
                if flight.answer.is_none() || down {
                    let flight = self.flight.remove(index);
                    self.end(at, flight, Ended::Failed);
                    continue;
                }
                flight.receiving = true;
            }
            if flight.receiving && unlimited {
                flight.left = 0;
            }
            if flight.receiving && flight.left == 0 {
                let mut flight = self.flight.remove(index);
                let answer = flight.answer.take().expect("an answer");
                self.end(at, flight, Ended::Answered(answer));
                continue;
            }
            index += 1;
        }
    }

    /// Carries the transfers to `to`.
    fn advance(&mut self, origin: &dyn Origin, to: u64) {
        let flight = std::mem::take(&mut self.flight);
        for flight in flight {
            if flight.transfer.is_cancelled() {
                self.counts.cancelled += 1;
                let received = flight.received();
                self.counts.bytes += received;
                self.counts.wasted += received;
            } else {
                self.flight.push(flight);
            }
        }
        self.take_started(origin);
        let bandwidth = self.profile.bandwidth.map(u128::from);
        let mut at = self.now;
        loop {
            self.events_at(at);
            if at >= to {
                break;
            }
            let receiving = self.flight.iter().filter(|flight| flight.receiving).count() as u128;
            let first_byte =
                self.flight.iter().filter(|flight| !flight.receiving).map(|flight| flight.first_byte).min();
            let last_byte = bandwidth.filter(|_| receiving > 0).map(|bandwidth| {
                let left = self.flight.iter().filter(|flight| flight.receiving).map(|flight| flight.left).min();
                at + (left.expect("a transfer receiving") * receiving).div_ceil(bandwidth) as u64
            });
            let outage = self.outage().map(|(from, _)| from).filter(|&from| from > at);
            let next = [first_byte, last_byte, outage, Some(to)].into_iter().flatten().min().expect("the end");
            if let Some(bandwidth) = bandwidth.filter(|_| receiving > 0) {
                let each = bandwidth * u128::from(next - at) / receiving;
                for flight in self.flight.iter_mut().filter(|flight| flight.receiving) {
                    flight.left -= flight.left.min(each);
                }
            }
            at = next;
        }
        self.now = to;
        self.ended.sort_by_key(|(at, seq, ..)| (*at, *seq));
    }
}

/// The network, and the online file source over it.
pub(crate) struct Network {
    simulator: Arc<Simulator>,
    driver: OnlineDriver,
    source: Arc<dyn FileSource>,
}

impl Network {
    /// A network over `profile` to `server`, its clock at zero.
    pub(crate) fn new(server: Arc<Server>, profile: Profile) -> Network {
        Network::to(server, profile)
    }

    fn to(origin: Arc<dyn Origin>, profile: Profile) -> Network {
        let simulator = Arc::new(Simulator { origin, state: Mutex::new(State::new(profile)) });
        let base = Instant::now();
        let clock: Clock = {
            let simulator = Arc::clone(&simulator);
            Arc::new(move || base + Duration::from_nanos(simulator.state.lock().now))
        };
        let (source, driver) = online_file_source(&Arc::default(), Arc::<Simulator>::clone(&simulator), clock);
        Network { simulator, driver, source: Arc::new(source) }
    }

    /// A loader over the network, and `cache` before it, reading on `pool`.
    pub(crate) fn file_source(&self, pool: &Pool, cache: Option<Arc<dyn Cache>>) -> Arc<dyn FileSource> {
        let builder = Loader::builder(pool).network(Arc::clone(&self.source));
        let builder = match cache {
            Some(cache) => builder.database(cache),
            None => builder,
        };
        Arc::new(builder.build())
    }

    /// Carries the transfers from now on over `profile`.
    pub(crate) fn set_profile(&self, profile: Profile) {
        self.simulator.state.lock().profile = profile;
    }

    /// Starts the faults' clock: failures and outages happen from now on.
    pub(crate) fn begin_faults(&self) {
        let mut state = self.simulator.state.lock();
        state.faults_from = Some(state.now);
    }

    /// Moves the network's clock a frame on, and carries the transfers to
    /// then. Their answers wait for [`pump`](Self::pump).
    pub(crate) fn advance(&self) {
        let mut state = self.simulator.state.lock();
        let to = state.now + FRAME;
        state.advance(&*self.simulator.origin, to);
    }

    /// Hands the transfers that ended to the online file source, and runs
    /// it: its requests, cancellations and deadlines. Returns whether it
    /// had anything to do.
    pub(crate) fn pump(&mut self) -> bool {
        let ended = std::mem::take(&mut self.simulator.state.lock().ended);
        let delivered = !ended.is_empty();
        for (_, _, transfer, ended) in ended {
            match ended {
                Ended::Answered(answer) => transfer.respond(answer.status, answer.headers, answer.body),
                Ended::Failed => transfer.fail("the network is down"),
            }
        }
        self.driver.run() || delivered
    }

    /// What has gone over the network so far, the bytes of the transfers
    /// still receiving included.
    pub(crate) fn counts(&self) -> Counts {
        let state = self.simulator.state.lock();
        let receiving: u64 = state.flight.iter().map(InFlight::received).sum();
        Counts { bytes: state.counts.bytes + receiving, ..state.counts }
    }

    /// Whether no transfer is in flight.
    pub(crate) fn is_idle(&self) -> bool {
        let state = self.simulator.state.lock();
        state.started.is_empty() && state.flight.is_empty() && state.ended.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use mln_storage::{Request, Response};

    use super::*;

    /// Answers `https://t/<n>` with a body of `n` bytes.
    struct Sized;

    impl Origin for Sized {
        fn answer(&self, resource: &Resource) -> Answer {
            let size: usize = resource.url.rsplit('/').next().and_then(|n| n.parse().ok()).expect("a size");
            Answer { status: 200, headers: Vec::new(), body: vec![0; size], wire: HEADER_BYTES + size as u64 }
        }
    }

    /// 100 ms to the first byte, then 13,000 bytes a second: a body of
    /// 1,000 bytes and its headers take another 100 ms alone.
    const LINK: Profile = Profile::link(100, Some(13_000));

    /// The answers to a request.
    type Answers = Arc<Mutex<Vec<Response>>>;

    fn request(network: &Network, url: &str) -> (Request, Answers) {
        let answers = Answers::default();
        let saved = Arc::clone(&answers);
        let (request, responder) = Request::new(move |response| saved.lock().push(response));
        network.source.load(&Resource::source(url), responder);
        (request, answers)
    }

    /// Runs frames until `answers` has `n` answers, and returns at which.
    fn frames_until(network: &mut Network, answers: &Answers, n: usize) -> u64 {
        for frame in 0..1000 {
            network.pump();
            if answers.lock().len() >= n {
                return frame;
            }
            network.advance();
        }
        panic!("no answer");
    }

    #[test]
    fn a_transfer_takes_a_round_trip_and_its_share_of_the_bandwidth() {
        let mut network = Network::to(Arc::new(Sized), LINK);
        let (_one, answers) = request(&network, "https://t/1000");
        // 200 ms: the frame of 200.000004 ms.
        assert_eq!(frames_until(&mut network, &answers, 1), 12);
        assert_eq!(answers.lock()[0].data.as_ref().map(|data| data.len()), Some(1000));

        // Two at once take twice as long to receive: 300 ms.
        let mut network = Network::to(Arc::new(Sized), LINK);
        let (_first, first) = request(&network, "https://t/a/1000");
        let (_second, second) = request(&network, "https://t/b/1000");
        assert_eq!(frames_until(&mut network, &first, 1), 18);
        assert_eq!(second.lock().len(), 1, "both at once");
        assert_eq!(network.counts(), Counts { requests: 2, bytes: 2600, ..Counts::default() });
    }

    #[test]
    fn an_outage_fails_the_transfers_until_the_backoff_retries_after_it() {
        let profile = Profile {
            outage: Some((Duration::from_millis(500), Duration::from_millis(2_000))),
            ..Profile::link(100, None)
        };
        let mut network = Network::to(Arc::new(Sized), profile);
        network.begin_faults();
        for _ in 0..27 {
            network.pump();
            network.advance();
        }
        // In flight at 450 ms when the outage starts at 500 ms: it fails
        // then. The online file source retries after a second, at 1.5 s,
        // which fails too, and after two more, at 3.6 s, whose answer comes
        // at 3.7 s: the frame of 3,700.00007 ms.
        let (_request, answers) = request(&network, "https://t/10");
        assert_eq!(frames_until(&mut network, &answers, 3), 222 - 27);
        let answers = answers.lock();
        assert!(answers[0].error.is_some() && answers[1].error.is_some());
        assert_eq!(answers[2].data.as_ref().map(|data| data.len()), Some(10));
        let counts = network.counts();
        assert_eq!((counts.requests, counts.retries, counts.wasted), (3, 2, 0));
    }

    #[test]
    fn a_server_error_is_retried_after_a_second() {
        let profile = Profile { server_errors: 1000, ..Profile::link(0, None) };
        let mut network = Network::to(Arc::new(Sized), profile);
        network.begin_faults();
        let (_request, answers) = request(&network, "https://t/10");
        // Each 503 comes the frame after its transfer starts, and the next
        // starts a second after it came: 503s at frames 1, 62 and 123.
        assert_eq!(frames_until(&mut network, &answers, 3), 123);
        assert!(answers.lock().iter().all(|answer| answer.error.is_some()));
        let counts = network.counts();
        assert_eq!(
            (counts.requests, counts.retries, counts.bytes, counts.wasted),
            (3, 2, 3 * HEADER_BYTES, 3 * HEADER_BYTES)
        );
    }

    #[test]
    fn a_cancelled_transfer_wastes_what_it_received() {
        let mut network = Network::to(Arc::new(Sized), LINK);
        let (request, answers) = request(&network, "https://t/1000");
        // At 150 ms, half of its 1,300 bytes have come.
        for _ in 0..9 {
            network.pump();
            network.advance();
        }
        assert_eq!(network.counts().bytes, 650, "what has come so far");
        drop(request);
        network.pump();
        network.advance();
        assert!(network.is_idle());
        assert!(answers.lock().is_empty());
        assert_eq!(
            network.counts(),
            Counts { requests: 1, cancelled: 1, bytes: 650, wasted: 650, ..Counts::default() }
        );
    }

    #[test]
    fn faults_depend_on_the_transfer_alone() {
        let state = State { faults_from: Some(0), ..State::new(LOCAL) };
        let fails = |attempt: u64| -> Vec<bool> {
            (0..200).map(|n| state.draws(&format!("https://t/{n}"), attempt, 0, 100)).collect()
        };
        assert_eq!(fails(1), fails(1));
        assert_ne!(fails(1), fails(2), "a retry draws anew");
        let failed = fails(1).iter().filter(|&&fails| fails).count();
        assert!((10..=30).contains(&failed), "{failed} of 200 at 10%");
        let before = State::new(LOCAL);
        assert!(!before.draws("https://t/1", 1, 0, 1000), "no faults before they begin");
    }
}
