//! The Java VM that the crates' Android services call: HTTP through
//! `HttpURLConnection`, and `android.icu` for the locale-sensitive
//! expressions.

#![expect(unsafe_code, reason = "dlsym and the JNI invocation API are the FFI here")]

use std::ffi::c_void;

use jni::JavaVM;
use jni::sys::{JNI_OK, jint, jsize};

/// The Java VM: the one that Android initialization recorded as `jni`'s
/// (`mln_android_init()`, or `mln_engine::storage`'s and `mln_system_fonts`' `init`),
/// or else the process's own, such as an app's that didn't initialize the
/// library, or that of a test that runs in a Java VM's process. `None` in a
/// process without one.
#[must_use]
pub fn java_vm() -> Option<JavaVM> {
    JavaVM::singleton().ok().or_else(process_java_vm)
}

/// The process's Java VM, through `JNI_GetCreatedJavaVMs`, which
/// libnativehelper exports to apps from API 31. A process with a Java VM has
/// loaded libnativehelper, so this doesn't load it (`RTLD_NOLOAD`).
fn process_java_vm() -> Option<JavaVM> {
    type GetCreatedJavaVms = unsafe extern "system" fn(*mut *mut jni::sys::JavaVM, jsize, *mut jsize) -> jint;
    // SAFETY: a library name and flags that dlopen takes.
    let library = unsafe { libc::dlopen(c"libnativehelper.so".as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD) };
    if library.is_null() {
        return None;
    }
    // SAFETY: the loaded library, and a symbol name.
    let symbol = unsafe { libc::dlsym(library, c"JNI_GetCreatedJavaVMs".as_ptr()) };
    if symbol.is_null() {
        return None;
    }
    // SAFETY: JNI_GetCreatedJavaVMs has this signature.
    let get_created_java_vms = unsafe { std::mem::transmute::<*mut c_void, GetCreatedJavaVms>(symbol) };
    let mut vm = std::ptr::null_mut();
    let mut count = 0;
    // SAFETY: room for one VM, and the count.
    let status = unsafe { get_created_java_vms(&raw mut vm, 1, &raw mut count) };
    // SAFETY: the process's live VM, which it keeps until it exits.
    (status == JNI_OK && count == 1 && !vm.is_null()).then(|| unsafe { JavaVM::from_raw(vm) })
}
