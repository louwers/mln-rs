//! A video's bytes, read through the map's storage.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use mln_storage::{Data, FileSource, LoadingMethod, Request, Resource, Response};
use parking_lot::Mutex;

/// The bytes a chunk read asks for. Players read a little at a time, so each
/// chunk serves many reads, and the cache keys chunks of fixed ranges.
const CHUNK_SIZE: u64 = 512 * 1024;
/// The chunks kept in memory, most recently read last.
const CACHED_CHUNKS: usize = 8;
/// How often a blocked read checks whether the source was cancelled.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// A video resource that players read like a file, from any thread but the
/// map's, in aligned chunks fetched through a [`FileSource`] with range
/// requests, as `Kind::Source` resources (GL JS's `transformRequest` sees
/// video URLs as sources). So the map's URL transforms, header rules,
/// provider routes and cache apply to video too.
pub struct ByteSource {
    file_source: Arc<dyn FileSource>,
    url: String,
    state: Mutex<State>,
    cancelled: AtomicBool,
}

#[derive(Default)]
struct State {
    /// The resource's size: `None` until a response said, then `Some(None)`
    /// when no source knows it.
    size: Option<Option<u64>>,
    chunks: Vec<(u64, Data)>,
}

impl std::fmt::Debug for ByteSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ByteSource").field("url", &self.url).finish_non_exhaustive()
    }
}

impl ByteSource {
    #[must_use]
    pub fn new(file_source: Arc<dyn FileSource>, url: impl Into<String>) -> ByteSource {
        ByteSource { file_source, url: url.into(), state: Mutex::default(), cancelled: AtomicBool::new(false) }
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The resource's size in bytes, or `None` when no source knows it (a
    /// cached response, and no network): then reads end at the first short
    /// chunk.
    ///
    /// # Errors
    ///
    /// The resource's first chunk failed to load.
    pub fn size(&self) -> io::Result<Option<u64>> {
        if let Some(size) = self.state.lock().size {
            return Ok(size);
        }
        let chunk = self.chunk(0)?;
        if let Some(size) = self.state.lock().size {
            return Ok(size);
        }
        // A cached first chunk doesn't know the size; the network does, and
        // a short chunk is the whole resource.
        let size = self.fetch(0, LoadingMethod::NetworkOnly).ok().and_then(|(_, size)| size);
        let short = (chunk.len() as u64) < CHUNK_SIZE;
        let size = size.or(short.then_some(chunk.len() as u64));
        Ok(*self.state.lock().size.get_or_insert(size))
    }

    /// Reads bytes from `offset` into `buf`, as many as the chunk at
    /// `offset` has: zero at the end of the resource. It blocks until the
    /// chunk arrives, so it must not run on the map's thread.
    ///
    /// # Errors
    ///
    /// The chunk failed to load, or the source was cancelled.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if let Some(Some(size)) = self.state.lock().size
            && offset >= size
        {
            return Ok(0);
        }
        let index = offset / CHUNK_SIZE;
        let chunk = self.chunk(index)?;
        let start = (offset - index * CHUNK_SIZE) as usize;
        let Some(available) = chunk.get(start..) else {
            return Ok(0);
        };
        let count = available.len().min(buf.len());
        buf[..count].copy_from_slice(&available[..count]);
        Ok(count)
    }

    /// Fills `buf` from `offset`, across chunks, up to the end of the
    /// resource; the count read.
    ///
    /// # Errors
    ///
    /// As [`read_at`](Self::read_at).
    pub fn read_full(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut filled = 0;
        while filled < buf.len() {
            let count = self.read_at(offset + filled as u64, &mut buf[filled..])?;
            if count == 0 {
                break;
            }
            filled += count;
        }
        Ok(filled)
    }

    /// Ends every read, now and later, with an error: the player closes.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Chunk `index`, from memory or storage, cut at the resource's end.
    fn chunk(&self, index: u64) -> io::Result<Data> {
        {
            let mut state = self.state.lock();
            if let Some(position) = state.chunks.iter().position(|(cached, _)| *cached == index) {
                let entry = state.chunks.remove(position);
                let data = Arc::clone(&entry.1);
                state.chunks.push(entry);
                return Ok(data);
            }
        }
        let (mut data, size) = self.fetch(index * CHUNK_SIZE, LoadingMethod::All)?;
        let mut state = self.state.lock();
        if size.is_some() {
            state.size = Some(size);
        }
        // A local file pads a range past its end with zeros.
        if let Some(Some(size)) = state.size {
            let end = size.saturating_sub(index * CHUNK_SIZE);
            if (data.len() as u64) > end {
                data = Arc::new(data[..end as usize].to_vec());
            }
        }
        if state.chunks.len() == CACHED_CHUNKS {
            state.chunks.remove(0);
        }
        state.chunks.push((index, Arc::clone(&data)));
        Ok(data)
    }

    /// One response for the chunk at `offset`, loaded by `method`: its data
    /// and the resource's size, if the response knows it.
    fn fetch(&self, offset: u64, method: LoadingMethod) -> io::Result<(Data, Option<u64>)> {
        let mut resource = Resource::source(&self.url);
        resource.data_range = Some((offset, offset + CHUNK_SIZE - 1));
        resource.loading_method = method;
        let (sender, receiver) = mpsc::sync_channel::<Response>(1);
        let (request, responder) = Request::direct_once(move |response| {
            let _ = sender.try_send(response);
        });
        self.file_source.load(&resource, responder);
        let response = loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "the video closed"));
            }
            match receiver.recv_timeout(CANCEL_POLL) {
                Ok(response) => break response,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other(format!("{} has no response", self.url)));
                }
            }
        };
        drop(request);
        if let Some(error) = response.error {
            return Err(io::Error::other(format!("{}: {}", self.url, error.message)));
        }
        let data = response.data.unwrap_or_default();
        Ok((data, response.total_size))
    }
}

#[cfg(test)]
mod tests {
    use mln_storage::Responder;

    use super::*;

    /// Serves `bytes` as a local file would: ranges padded with zeros, and
    /// the size known, or, `cached`, as a cache would: no size.
    struct Bytes {
        bytes: Vec<u8>,
        cached: bool,
    }

    impl FileSource for Bytes {
        fn load(&self, resource: &Resource, responder: Responder) {
            let (first, last) = resource.data_range.unwrap();
            let network = resource.loading_method == LoadingMethod::NetworkOnly;
            let mut data: Vec<u8> =
                self.bytes.iter().copied().skip(first as usize).take((last - first + 1) as usize).collect();
            if !self.cached {
                data.resize((last - first + 1) as usize, 0);
            }
            let response = Response {
                data: Some(Arc::new(data)),
                total_size: (!self.cached || network).then_some(self.bytes.len() as u64),
                ..Response::default()
            };
            std::thread::spawn(move || responder.send(response));
        }

        fn can_request(&self, _resource: &Resource) -> bool {
            true
        }
    }

    fn bytes(length: usize) -> Vec<u8> {
        (0..length).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn reads_cross_chunks_and_stop_at_the_end() {
        let content = bytes(CHUNK_SIZE as usize * 2 + 100);
        let source = ByteSource::new(Arc::new(Bytes { bytes: content.clone(), cached: false }), "file://video");
        assert_eq!(Some(content.len() as u64), source.size().unwrap());
        let mut buf = vec![0; 300];
        let offset = CHUNK_SIZE - 50;
        assert_eq!(300, source.read_full(offset, &mut buf).unwrap());
        assert_eq!(&content[offset as usize..offset as usize + 300], &buf[..]);
        let end = content.len() as u64;
        assert_eq!(40, source.read_full(end - 40, &mut buf).unwrap());
        assert_eq!(0, source.read_at(end, &mut buf).unwrap());
    }

    #[test]
    fn a_cached_resource_learns_its_size_from_the_network() {
        let content = bytes(1000);
        let source = ByteSource::new(Arc::new(Bytes { bytes: content, cached: true }), "https://video");
        assert_eq!(Some(1000), source.size().unwrap());
    }

    #[test]
    fn a_cancelled_source_stops_reading() {
        struct Never;
        impl FileSource for Never {
            fn load(&self, _resource: &Resource, responder: Responder) {
                std::mem::forget(responder);
            }
            fn can_request(&self, _resource: &Resource) -> bool {
                true
            }
        }
        let source = Arc::new(ByteSource::new(Arc::new(Never), "https://video"));
        let reader = Arc::clone(&source);
        let read = std::thread::spawn(move || reader.read_at(0, &mut [0; 4]));
        std::thread::sleep(Duration::from_millis(100));
        source.cancel();
        assert_eq!(io::ErrorKind::Interrupted, read.join().unwrap().unwrap_err().kind());
    }
}
