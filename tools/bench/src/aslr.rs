//! A scenario's process, laid out at the same addresses in every run.
//!
//! Where the system places a process's code, stack and heap at random, the
//! instructions that copies, comparisons and searches take change with the
//! addresses they work on, so a scenario's instruction count shifts from
//! one process to the next while each repeats exactly within a process.
//! A scenario's process therefore runs its binary again, once, with the
//! randomization off, where the system lets it: macOS through
//! `posix_spawn`'s flag for it, Linux through `personality(2)`. Elsewhere,
//! or where the system refuses, it runs as it is.

#![cfg_attr(
    any(target_os = "macos", target_os = "linux", target_os = "android"),
    expect(unsafe_code, reason = "posix_spawn and personality are FFI")
)]

/// Set in the environment of the process that runs again, so that it
/// doesn't run again itself.
#[cfg(target_os = "macos")]
const AGAIN: &str = "MLN_BENCH_FIXED_LAYOUT";

/// Replaces this process with its binary run again with the same arguments
/// and the randomization off, unless it already runs that way. Returns
/// where it doesn't replace it.
pub(crate) fn fix() {
    #[cfg(target_os = "macos")]
    macos::fix();
    #[cfg(any(target_os = "linux", target_os = "android"))]
    linux::fix();
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{CString, OsString};
    use std::os::unix::ffi::OsStringExt as _;

    use super::AGAIN;

    /// `_POSIX_SPAWN_DISABLE_ASLR` of `<spawn.h>`, private to Apple's
    /// headers, which debuggers use.
    const DISABLE_ASLR: libc::c_int = 0x0100;

    pub(super) fn fix() {
        if std::env::var_os(AGAIN).is_some() {
            return;
        }
        let Ok(binary) = std::env::current_exe() else { return };
        let strings = |values: Vec<OsString>| -> Option<Vec<CString>> {
            values.into_iter().map(|value| CString::new(value.into_vec()).ok()).collect()
        };
        let Some(path) = strings(vec![binary.into_os_string()]).and_then(|mut path| path.pop()) else { return };
        let Some(args) = strings(std::env::args_os().collect()) else { return };
        let mut environment: Vec<OsString> = std::env::vars_os()
            .map(|(name, value)| {
                let mut entry = name;
                entry.push("=");
                entry.push(value);
                entry
            })
            .collect();
        environment.push(format!("{AGAIN}=1").into());
        let Some(environment) = strings(environment) else { return };
        let pointers = |strings: &[CString]| -> Vec<*mut libc::c_char> {
            strings.iter().map(|string| string.as_ptr().cast_mut()).chain([std::ptr::null_mut()]).collect()
        };
        let (argv, envp) = (pointers(&args), pointers(&environment));

        let mut attributes: libc::posix_spawnattr_t = std::ptr::null_mut();
        // SAFETY: `attributes` is initialized before use and destroyed after;
        // every string is NUL-terminated and outlives the call, and `argv`
        // and `envp` end in a null pointer. With POSIX_SPAWN_SETEXEC, a
        // `posix_spawn` that succeeds replaces this process and doesn't
        // return.
        unsafe {
            if libc::posix_spawnattr_init(&raw mut attributes) != 0 {
                return;
            }
            let flags = (libc::POSIX_SPAWN_SETEXEC | DISABLE_ASLR) as libc::c_short;
            if libc::posix_spawnattr_setflags(&raw mut attributes, flags) == 0 {
                let mut pid: libc::pid_t = 0;
                libc::posix_spawn(
                    &raw mut pid,
                    path.as_ptr(),
                    std::ptr::null(),
                    &raw const attributes,
                    argv.as_ptr(),
                    envp.as_ptr(),
                );
            }
            libc::posix_spawnattr_destroy(&raw mut attributes);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod linux {
    use std::os::unix::process::CommandExt as _;
    use std::process::Command;

    pub(super) fn fix() {
        // SAFETY: personality only reads its argument, and this one, its
        // query of the current persona, changes nothing. (Its type is
        // `unsigned long` on Linux and `unsigned int` on Android.)
        let persona = unsafe { libc::personality(0xffff_ffff) };
        if persona < 0 || persona & libc::ADDR_NO_RANDOMIZE != 0 {
            return;
        }
        // SAFETY: as above; the persona applies to the program exec runs.
        if unsafe { libc::personality((persona | libc::ADDR_NO_RANDOMIZE) as _) } < 0 {
            return;
        }
        let Ok(binary) = std::env::current_exe() else { return };
        // Returns only if the binary can't run.
        let _ = Command::new(binary).args(std::env::args_os().skip(1)).exec();
    }
}
