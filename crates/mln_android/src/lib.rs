//! What the library's Android code shares: the Java VM that its services
//! call, such as HTTP through `HttpURLConnection` and the locale-sensitive
//! expressions through `android.icu`. Empty on other platforms.

#[cfg(target_os = "android")]
mod java_vm;

#[cfg(target_os = "android")]
pub use java_vm::java_vm;
