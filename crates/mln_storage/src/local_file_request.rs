use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use mln_actor::ActorRef;

use crate::file_source_request::FileSourceRequestState;
use crate::response::{Reason, ResourceError, Response};

/// The path bytes as they are (POSIX paths are bytes; on other platforms
/// invalid UTF-8 is an error, as `std::filesystem`'s UTF-8 conversion throws
/// there).
#[cfg_attr(unix, expect(clippy::unnecessary_wraps, reason = "only a non-Unix path can fail to convert"))]
pub(crate) fn path_from_utf8(path: &[u8]) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(path)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(path.to_vec()).map(PathBuf::from).map_err(|_| "Invalid UTF-8 in path".to_owned())
    }
}

/// The whole file, or `data_range`'s bytes (`end - start + 1` of them,
/// zero-padded past the end as in C++), or `None` when the file can't be
/// opened. A range too large to allocate (e.g. from a corrupt PMTiles
/// header) is `None` too; C++ throws `std::length_error` or `std::bad_alloc`,
/// uncaught.
#[must_use]
pub(crate) fn read_file(path: &std::path::Path, data_range: Option<(u64, u64)>) -> Option<Vec<u8>> {
    read_range(std::fs::File::open(path).ok()?, data_range)
}

/// [`read_file`]'s read of an open file, or of anything else that reads and
/// seeks like one.
pub(crate) fn read_range(mut file: impl Read + Seek, data_range: Option<(u64, u64)>) -> Option<Vec<u8>> {
    if let Some((first, second)) = data_range {
        let size = second.wrapping_sub(first).wrapping_add(1);
        let mut data = Vec::new();
        data.try_reserve_exact(usize::try_from(size).ok()?).ok()?;
        if file.seek(SeekFrom::Start(first)).is_ok() {
            // A read error stops the read, as in C++.
            let _ = (&mut file).take(size).read_to_end(&mut data);
        }
        data.resize(size as usize, 0);
        Some(data)
    } else {
        let mut data = Vec::new();
        // `data << file.rdbuf()`: a read error (e.g. a directory) stops
        // the copy.
        let _ = file.read_to_end(&mut data);
        Some(data)
    }
}

pub(crate) fn request_local_file(path: &[u8], req: &ActorRef<FileSourceRequestState>, data_range: Option<(u64, u64)>) {
    let mut response = Response::new();
    let path_display = String::from_utf8_lossy(path).into_owned();
    let path = match path_from_utf8(path) {
        Ok(path) => path,
        Err(message) => {
            // `catch (const std::filesystem::filesystem_error& exception)`.
            response.error = Some(Box::new(ResourceError::new(Reason::Other, message)));
            req.invoke(move |request: &FileSourceRequestState| request.set_response(response));
            return;
        }
    };

    // `std::filesystem::status(path, error)`: a failure other than "not
    // found" (e.g. a name that is too long) is `file_type::none`.
    let status = std::fs::metadata(&path);
    let not_found = matches!(&status, Err(e) if e.kind() == std::io::ErrorKind::NotFound);
    let is_directory = matches!(&status, Ok(metadata) if metadata.is_dir());

    if not_found {
        response.error =
            Some(Box::new(ResourceError::new(Reason::NotFound, format!("Cannot find file {path_display}"))));
    } else if is_directory {
        response.error =
            Some(Box::new(ResourceError::new(Reason::NotFound, format!("Cannot read directory {path_display}"))));
    } else {
        match read_file(&path, data_range) {
            None => {
                response.error =
                    Some(Box::new(ResourceError::new(Reason::Other, format!("Cannot read file {path_display}"))));
            }
            Some(data) => response.data = Some(Arc::new(data)),
        }
    }

    req.invoke(move |request: &FileSourceRequestState| request.set_response(response));
}
