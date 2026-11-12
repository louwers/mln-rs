//! What the file sources need of the Android app that they run in: HTTPS
//! checks certificates with Android's own verifier, which runs in the app's
//! Java VM.

// JNI is the FFI here.
#![allow(unsafe_code)]

use std::ffi::c_void;

use jni::errors::Error as JniError;
use jni::objects::JObject;
use jni::{EnvUnowned, Outcome};
use mln_util::error::Error;

/// Gives the file sources the Java VM of `jni_env` and the app's `context`.
/// Call it before the first HTTPS request; a later call changes nothing.
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
            let initialized = rustls_platform_verifier::android::init_with_env(env, context);
            if env.exception_check() {
                env.exception_clear();
            }
            initialized
        })
        .into_outcome();
    match outcome {
        Outcome::Ok(()) => Ok(()),
        Outcome::Err(error) => Err(Error::native(format!("Android's certificate verifier failed to start: {error}"))),
        Outcome::Panic(_) => Err(Error::native("Android's certificate verifier panicked")),
    }
}
