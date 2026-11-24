//! The Java VM that the library's Android services call: HTTP through
//! `HttpURLConnection`, and `android.icu` for the locale-sensitive
//! expressions.

#![expect(unsafe_code, reason = "dlsym and the JNI invocation API are the FFI here")]

use std::ffi::{c_char, c_int, c_void};

use jni::JavaVM;
use jni::sys::{JNI_OK, jint, jsize};

/// The Java VM: the one that Android initialization gave the library, or
/// else the process's own, such as an app's that didn't initialize it, or
/// that of a test that runs inside a Java VM's process. `None` in a process
/// without one.
#[must_use]
pub fn java_vm() -> Option<JavaVM> {
    JavaVM::singleton().ok().or_else(|| {
        let vm = process_java_vm()?;
        // SAFETY: the process's live VM, which a process keeps until it exits.
        Some(unsafe { JavaVM::from_raw(vm) })
    })
}

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;
/// Only a library that the process has loaded: a process with a Java VM has
/// loaded libnativehelper.
const RTLD_NOLOAD: c_int = 4;

/// The process's Java VM, through `JNI_GetCreatedJavaVMs`, which
/// libnativehelper exports to apps from API 31.
fn process_java_vm() -> Option<*mut jni::sys::JavaVM> {
    type GetCreatedJavaVms = unsafe extern "system" fn(*mut *mut jni::sys::JavaVM, jsize, *mut jsize) -> jint;
    // SAFETY: a library name and flags that dlopen takes.
    let library = unsafe { dlopen(c"libnativehelper.so".as_ptr(), RTLD_NOW | RTLD_NOLOAD) };
    if library.is_null() {
        return None;
    }
    // SAFETY: the loaded library, and a symbol name.
    let symbol = unsafe { dlsym(library, c"JNI_GetCreatedJavaVMs".as_ptr()) };
    if symbol.is_null() {
        return None;
    }
    // SAFETY: JNI_GetCreatedJavaVMs has this signature.
    let get_created_java_vms = unsafe { std::mem::transmute::<*mut c_void, GetCreatedJavaVms>(symbol) };
    let mut vm = std::ptr::null_mut();
    let mut count = 0;
    // SAFETY: room for one VM, and the count.
    let status = unsafe { get_created_java_vms(&raw mut vm, 1, &raw mut count) };
    (status == JNI_OK && count == 1 && !vm.is_null()).then_some(vm)
}
