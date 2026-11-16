//! On Windows, puts DXC's `dxcompiler.dll` beside the app, where the D3D12
//! backend loads it from to compile its shaders: from the directory that
//! `MLN_DXC_DIR` names, or else the newest Windows SDK's for the target's
//! architecture. DXC 1.8.2502 and later sign what they compile, so the app
//! needs no `dxil.dll`; an older DXC's `dxil.dll` comes along when it sits
//! beside it. Without a DXC, the app runs on the other backends, and the
//! build says so.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo::rerun-if-env-changed=MLN_DXC_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Some(dxc) = dxc_dir() else {
        println!(
            "cargo::warning=no dxcompiler.dll found (set MLN_DXC_DIR, or install the Windows SDK): D3D12 needs one on \
             the DLL search path"
        );
        return;
    };
    // OUT_DIR is target/<profile>/build/<package>-<hash>/out, beside the
    // profile's executables three levels up.
    let Some(app_dir) =
        std::env::var_os("OUT_DIR").map(PathBuf::from).and_then(|out| out.ancestors().nth(3).map(Path::to_path_buf))
    else {
        return;
    };
    for name in ["dxcompiler.dll", "dxil.dll"] {
        let source = dxc.join(name);
        if source.is_file() {
            println!("cargo::rerun-if-changed={}", source.display());
            if let Err(error) = std::fs::copy(&source, app_dir.join(name)) {
                println!("cargo::warning=copying {} beside the app failed: {error}", source.display());
            }
        }
    }
}

/// The directory of the `dxcompiler.dll` to ship.
fn dxc_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MLN_DXC_DIR") {
        return Some(PathBuf::from(dir)).filter(|dir| dir.join("dxcompiler.dll").is_file());
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").ok()?.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "x86" => "x86",
        _ => return None,
    };
    let kits = PathBuf::from(std::env::var_os("ProgramFiles(x86)")?).join("Windows Kits").join("10").join("bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(kits)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path().join(arch)))
        .filter(|dir| dir.join("dxcompiler.dll").is_file())
        .collect();
    // The SDK's versions are dotted numbers of the same width.
    versions.sort();
    versions.pop()
}
