//! What the file sources need of the Android app that they run in: HTTPS
//! checks certificates with Android's own verifier, which runs in the app's
//! Java VM, and `asset://` URLs load the app's assets, from its
//! `AssetManager`.

// JNI and the NDK's asset manager are the FFI here.
#![allow(unsafe_code)]

use std::ffi::{CString, c_int, c_void};
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, OnceLock};

use jni::errors::Error as JniError;
use jni::objects::{Global, JObject};
use jni::{EnvUnowned, Outcome, jni_sig, jni_str};
use mln_actor::ActorRef;
use mln_util::error::Error;

use crate::file_source_request::FileSourceRequestState;
use crate::local_file_request::read_range;
use crate::response::{Reason, ResourceError, Response};

/// The asset path whose files are the app's assets, which is the default on
/// Android: `asset://style.json` loads the app's `assets/style.json`.
pub const APP_ASSETS: &str = "/android_asset";

#[link(name = "android")]
unsafe extern "C" {
    fn AAssetManager_fromJava(env: *mut c_void, asset_manager: *mut c_void) -> *mut c_void;
    fn AAssetManager_open(manager: *mut c_void, name: *const std::ffi::c_char, mode: c_int) -> *mut c_void;
    fn AAsset_close(asset: *mut c_void);
    fn AAsset_read(asset: *mut c_void, buffer: *mut c_void, count: usize) -> c_int;
    fn AAsset_seek64(asset: *mut c_void, offset: i64, whence: c_int) -> i64;
}

/// `AASSET_MODE_RANDOM`.
const MODE_RANDOM: c_int = 1;

/// The app's asset manager, which the Java object that it comes from keeps
/// alive.
struct AssetManager {
    manager: usize,
    _object: Global<JObject<'static>>,
}

static ASSETS: OnceLock<AssetManager> = OnceLock::new();

/// Gives the file sources the Java VM of `jni_env` and the app's `context`.
/// Call it before the first HTTPS request or app asset; a later call changes
/// nothing.
///
/// The app must include rustls-platform-verifier's Android component, whose
/// classes verify the certificates.
///
/// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when JNI fails,
/// such as for a `context` that is no `Context`, and leaves no Java exception
/// pending.
///
/// # Safety
///
/// `jni_env` is the calling thread's `JNIEnv*`, and `context` a live
/// reference to an Android `Context`, such as the application.
pub unsafe fn init(jni_env: *mut c_void, context: *mut c_void) -> Result<(), Error> {
    // SAFETY: the caller's contract; the local references that JNI makes
    // here go with the caller's frame.
    let mut env = unsafe { EnvUnowned::from_raw(jni_env.cast()) };
    let outcome = env
        .with_env(|env| -> Result<(), JniError> {
            // SAFETY: the caller's reference, which nothing here deletes.
            let context = unsafe { JObject::from_raw(env, context.cast()) };
            let initialized = retain_assets(env, &context)
                .and_then(|()| rustls_platform_verifier::android::init_with_env(env, context));
            if env.exception_check() {
                env.exception_clear();
            }
            initialized
        })
        .into_outcome();
    match outcome {
        Outcome::Ok(()) => Ok(()),
        Outcome::Err(error) => Err(Error::native(format!("Android initialization failed: {error}"))),
        Outcome::Panic(_) => Err(Error::native("Android initialization panicked")),
    }
}

/// Keeps `context`'s asset manager, unless one is kept already.
fn retain_assets(env: &mut jni::Env<'_>, context: &JObject<'_>) -> Result<(), JniError> {
    if ASSETS.get().is_some() {
        return Ok(());
    }
    let object =
        env.call_method(context, jni_str!("getAssets"), jni_sig!("()Landroid/content/res/AssetManager;"), &[])?.l()?;
    let object = env.new_global_ref(object)?;
    // SAFETY: the thread's live environment, and a live AssetManager, which
    // the global reference keeps.
    let manager = unsafe { AAssetManager_fromJava(env.get_raw().cast(), object.as_obj().as_raw().cast()) };
    if manager.is_null() {
        return Err(JniError::NullPtr("AAssetManager_fromJava"));
    }
    let _ = ASSETS.set(AssetManager { manager: manager as usize, _object: object });
    Ok(())
}

/// An open asset, which reads and seeks as a file does.
struct Asset(*mut c_void);

impl Read for Asset {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        // SAFETY: the open asset, into `buffer`'s bytes.
        let read = unsafe { AAsset_read(self.0, buffer.as_mut_ptr().cast(), buffer.len()) };
        usize::try_from(read).map_err(|_| std::io::Error::other("the asset can't be read"))
    }
}

impl Seek for Asset {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let (offset, whence) = match position {
            SeekFrom::Start(offset) => {
                (i64::try_from(offset).map_err(|_| std::io::Error::other("past the asset"))?, libc_seek::SET)
            }
            SeekFrom::Current(offset) => (offset, libc_seek::CUR),
            SeekFrom::End(offset) => (offset, libc_seek::END),
        };
        // SAFETY: the open asset.
        let at = unsafe { AAsset_seek64(self.0, offset, whence) };
        u64::try_from(at).map_err(|_| std::io::Error::other("the asset can't seek there"))
    }
}

impl Drop for Asset {
    fn drop(&mut self) {
        // SAFETY: the asset is this one's.
        unsafe { AAsset_close(self.0) };
    }
}

/// `SEEK_SET`, `SEEK_CUR` and `SEEK_END`.
mod libc_seek {
    use std::ffi::c_int;

    pub(super) const SET: c_int = 0;
    pub(super) const CUR: c_int = 1;
    pub(super) const END: c_int = 2;
}

/// Answers `req` with the app's asset `name`, a path in its `assets`
/// directory, or `data_range`'s bytes of it, as `request_local_file` answers
/// with a file.
pub(crate) fn request_asset(name: &[u8], req: &ActorRef<FileSourceRequestState>, data_range: Option<(u64, u64)>) {
    let display = String::from_utf8_lossy(name).into_owned();
    let mut response = Response::new();
    let error = |reason, message: String| Some(Box::new(ResourceError::new(reason, message)));
    match (ASSETS.get(), CString::new(name.to_vec())) {
        (None, _) => {
            response.error =
                error(Reason::Other, "the app's assets are unknown before Android initialization".to_owned());
        }
        (_, Err(_)) => response.error = error(Reason::NotFound, format!("Cannot find asset {display}")),
        (Some(assets), Ok(name)) => {
            // SAFETY: the app's live asset manager, which any thread may use,
            // and a C string.
            let asset = unsafe { AAssetManager_open(assets.manager as *mut c_void, name.as_ptr(), MODE_RANDOM) };
            if asset.is_null() {
                response.error = error(Reason::NotFound, format!("Cannot find asset {display}"));
            } else {
                match read_range(Asset(asset), data_range) {
                    Some(data) => response.data = Some(Arc::new(data)),
                    None => response.error = error(Reason::Other, format!("Cannot read asset {display}")),
                }
            }
        }
    }
    req.invoke(move |request: &FileSourceRequestState| request.set_response(response));
}
