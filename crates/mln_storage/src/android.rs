//! What the file sources need of the Android app that they run in: HTTPS
//! checks certificates with Android's own verifier, which runs in the app's
//! Java VM, plain HTTP follows the app's network security configuration, and
//! `asset://` URLs load the app's assets, from its `AssetManager`.

// JNI and the NDK's asset manager are the FFI here.
#![allow(unsafe_code)]

use std::ffi::{CString, c_int, c_void};
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, OnceLock};

use jni::errors::Error as JniError;
use jni::objects::{Global, JObject, JValue};
use jni::{Env, EnvUnowned, JavaVM, Outcome, jni_sig, jni_str};
use mln_actor::ActorRef;
use mln_util::error::Error;
use mln_util::logging::{Event, Log};

use crate::file_source_request::FileSourceRequestState;
use crate::local_file_request::read_range;
use crate::response::{Reason, ResourceError, Response};

/// The asset path whose files are the app's assets, which is the default on
/// Android: `asset://style.json` loads the app's `assets/style.json`, as does
/// `file:///android_asset/style.json`.
pub const APP_ASSETS: &str = "/android_asset";

/// The name in the app's `assets` directory of the file at `path`, when
/// `path` is under [`APP_ASSETS`].
pub(crate) fn app_asset_name(path: &[u8]) -> Option<&[u8]> {
    let name = path.strip_prefix(APP_ASSETS.as_bytes())?.strip_prefix(b"/")?;
    // `asset:///style.json` names `style.json` too.
    Some(&name[name.iter().position(|byte| *byte != b'/').unwrap_or(name.len())..])
}

/// The path of a local URL after its scheme: what comes before any query or
/// fragment.
pub(crate) fn url_path(url: &str) -> &str {
    url.find(['?', '#']).map_or(url, |end| &url[..end])
}

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

/// Set once [`init`] gave rustls-platform-verifier the app's Java VM, and
/// found the verifier's classes in the app.
static VERIFIER: OnceLock<()> = OnceLock::new();

/// The app's Java VM, which [`init`] keeps.
static VM: OnceLock<JavaVM> = OnceLock::new();

/// The class of rustls-platform-verifier's Android component that verifies
/// the certificates.
const VERIFIER_CLASS: &str = "org.rustls.platformverifier.CertificateVerifier";

/// Whether [`init`] gave Android's certificate verifier the app's Java VM,
/// which the verifier panics without, and the app has the verifier's classes,
/// without which it fails every certificate.
pub(crate) fn verifier_ready() -> bool {
    VERIFIER.get().is_some()
}

/// Gives the file sources the Java VM of `jni_env` and the app's `context`.
/// Call it before the first HTTP request or app asset; a later call changes
/// nothing. Without it, HTTPS trusts the certificates of Android's store
/// files, which leave out the ones a user installed, and plain HTTP goes to
/// any host.
///
/// HTTPS verifies with Android's own verifier when the app includes
/// rustls-platform-verifier's Android component, whose classes call it, and
/// otherwise trusts the store files as without `init`.
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
        .with_env(|env| -> Result<bool, JniError> {
            // SAFETY: the caller's reference, which nothing here deletes.
            let context = unsafe { JObject::from_raw(env, context.cast()) };
            let initialized = (|| {
                retain_assets(env, &context)?;
                let _ = VM.set(env.get_java_vm()?);
                let has_verifier = has_class(env, &context, VERIFIER_CLASS);
                rustls_platform_verifier::android::init_with_env(env, context)?;
                Ok(has_verifier)
            })();
            if env.exception_check() {
                env.exception_clear();
            }
            initialized
        })
        .into_outcome();
    match outcome {
        Outcome::Ok(has_verifier) => {
            if has_verifier {
                let _ = VERIFIER.set(());
            } else {
                Log::warning(
                    Event::Android,
                    format!(
                        "the app has no {VERIFIER_CLASS} (rustls-platform-verifier's Android component), so HTTPS \
                         trusts the system's certificates alone"
                    ),
                );
            }
            Ok(())
        }
        Outcome::Err(error) => Err(Error::native(format!("Android initialization failed: {error}"))),
        Outcome::Panic(_) => Err(Error::native("Android initialization panicked")),
    }
}

/// Whether `context`'s class loader loads the class `name`. Leaves no Java
/// exception pending.
fn has_class(env: &mut Env<'_>, context: &JObject<'_>, name: &str) -> bool {
    let loaded = (|| -> Result<(), JniError> {
        let loader =
            env.call_method(context, jni_str!("getClassLoader"), jni_sig!("()Ljava/lang/ClassLoader;"), &[])?.l()?;
        let name = env.new_string(name)?;
        env.call_method(
            &loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::from(&name)],
        )?;
        Ok(())
    })();
    if env.exception_check() {
        env.exception_clear();
    }
    loaded.is_ok()
}

/// Whether the app's network security configuration permits plain HTTP to
/// `host`. Before [`init`], or when Java can't tell, any host is permitted.
pub(crate) fn cleartext_permitted(host: &str) -> bool {
    let Some(vm) = VM.get() else {
        return true;
    };
    // The thread stays attached, so a transport's next request is cheaper.
    vm.attach_current_thread(|env| -> Result<bool, JniError> {
        let class = env.find_class(jni_str!("android/security/NetworkSecurityPolicy"))?;
        let policy = env
            .call_static_method(
                &class,
                jni_str!("getInstance"),
                jni_sig!("()Landroid/security/NetworkSecurityPolicy;"),
                &[],
            )?
            .l()?;
        let host = env.new_string(host)?;
        env.call_method(
            &policy,
            jni_str!("isCleartextTrafficPermitted"),
            jni_sig!("(Ljava/lang/String;)Z"),
            &[JValue::from(&host)],
        )?
        .z()
    })
    .unwrap_or(true)
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
