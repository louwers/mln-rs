//! What a run's numbers depend on besides the code: the machine, the GPU a
//! scenario drew on, and the fixtures it read. `mln-bench compare` gives
//! verdicts only between runs whose environments are equal.

#![cfg_attr(target_os = "macos", expect(unsafe_code, reason = "sysctl is FFI"))]

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Environment {
    /// The operating system and its version.
    pub(crate) os: String,
    pub(crate) arch: String,
    /// The CPU's model.
    pub(crate) cpu: String,
    /// How the process counted instructions, if it could.
    pub(crate) instruction_counter: Option<String>,
    /// The GPU a scenario drew on; none for a scenario that draws nothing.
    pub(crate) gpu: Option<Gpu>,
    /// The hash of the fixture manifest a scenario read; none for one that
    /// reads no fixtures.
    pub(crate) fixtures: Option<String>,
}

/// A device a scenario drew on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Gpu {
    /// The adapter's or device's name, as its driver gives it.
    pub(crate) name: String,
    /// The API the renderer drew through, and for WebGPU the one under it.
    pub(crate) backend: String,
    /// Whether it rasterizes on the CPU, such as lavapipe or WARP. Its
    /// numbers are compared only with another CPU rasterizer's.
    pub(crate) software: bool,
}

impl Environment {
    /// The machine's part of the environment.
    pub(crate) fn machine() -> Environment {
        Environment {
            os: os(),
            arch: std::env::consts::ARCH.to_owned(),
            cpu: cpu(),
            instruction_counter: crate::meter::instruction_counter().map(str::to_owned),
            gpu: None,
            fixtures: None,
        }
    }

    /// The fields in which `self` and `other` differ, as `name: a / b`.
    pub(crate) fn differences(&self, other: &Environment) -> Vec<String> {
        let mut differences = Vec::new();
        let mut check = |name: &str, a: String, b: String| {
            if a != b {
                differences.push(format!("{name}: {a} / {b}"));
            }
        };
        check("os", self.os.clone(), other.os.clone());
        check("arch", self.arch.clone(), other.arch.clone());
        check("cpu", self.cpu.clone(), other.cpu.clone());
        check(
            "instruction counter",
            format!("{:?}", self.instruction_counter),
            format!("{:?}", other.instruction_counter),
        );
        check("gpu", format!("{:?}", self.gpu), format!("{:?}", other.gpu));
        check("fixtures", format!("{:?}", self.fixtures), format!("{:?}", other.fixtures));
        differences
    }
}

impl std::fmt::Display for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}, {}", self.os, self.arch, self.cpu)?;
        if let Some(gpu) = &self.gpu {
            let software = if gpu.software { ", CPU rasterizer" } else { "" };
            write!(f, ", {} ({}{software})", gpu.name, gpu.backend)?;
        }
        if self.instruction_counter.is_none() {
            write!(f, ", no instruction counter")?;
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn sysctl(name: &str) -> Option<String> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut size = 0;
    // SAFETY: a null buffer asks for the value's size, which `size` receives.
    let status =
        unsafe { libc::sysctlbyname(name.as_ptr(), std::ptr::null_mut(), &raw mut size, std::ptr::null_mut(), 0) };
    if status != 0 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    // SAFETY: `buffer` holds `size` bytes, which the call fills at most.
    let status = unsafe {
        libc::sysctlbyname(name.as_ptr(), buffer.as_mut_ptr().cast(), &raw mut size, std::ptr::null_mut(), 0)
    };
    if status != 0 {
        return None;
    }
    buffer.truncate(size);
    Some(String::from_utf8_lossy(&buffer).trim_end_matches('\0').trim().to_owned())
}

#[cfg(target_os = "macos")]
fn os() -> String {
    format!("macos {}", sysctl("kern.osproductversion").unwrap_or_default())
}

#[cfg(target_os = "macos")]
fn cpu() -> String {
    sysctl("machdep.cpu.brand_string").unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn os() -> String {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    format!("{} {}", std::env::consts::OS, release.trim())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn cpu() -> String {
    let info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    info.lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            matches!(key.trim(), "model name" | "Hardware" | "CPU part").then(|| value.trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
fn os() -> String {
    std::env::consts::OS.to_owned()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
fn cpu() -> String {
    std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown".to_owned())
}
