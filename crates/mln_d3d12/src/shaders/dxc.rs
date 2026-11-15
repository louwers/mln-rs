// DXC, from the `dxcompiler.dll` of the DLL search path, loaded when the
// first compiler is made, as wgpu loads it. Where `dxil.dll` sits beside it,
// DXC validates and signs what it compiles.

use std::ffi::c_void;

use mln_glsl::Stage;
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::Graphics::Direct3D::Dxc::{
    CLSID_DxcCompiler, DXC_ARG_OPTIMIZATION_LEVEL3, DXC_CP_UTF8, DxcBuffer, IDxcBlob, IDxcCompiler3, IDxcResult,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{GUID, HRESULT, Interface, PCWSTR, s, w};

use super::hlsl::Hlsl;

/// `DxcCreateInstance`.
type CreateInstance = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

/// A loaded library, freed when dropped.
struct Library(HMODULE);

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: nothing of the library's is in use: `Dxc` releases its
        // compiler first.
        let _ = unsafe { FreeLibrary(self.0) };
    }
}

/// A DXC compiler.
pub(crate) struct Dxc {
    compiler: IDxcCompiler3,
    // After `compiler`, which is released first.
    _library: Library,
}

impl Dxc {
    /// Loads `dxcompiler.dll` and makes a compiler.
    pub(crate) fn load() -> Result<Dxc, String> {
        // SAFETY: the name is a valid string, and DXC's initialization is
        // safe to run.
        let library = Library(unsafe { LoadLibraryW(w!("dxcompiler.dll")) }.map_err(|error| format!("{error}"))?);
        // SAFETY: the library is loaded, and the name is a valid string.
        let create = unsafe { GetProcAddress(library.0, s!("DxcCreateInstance")) }
            .ok_or_else(|| "dxcompiler.dll has no DxcCreateInstance".to_owned())?;
        // SAFETY: `DxcCreateInstance` has this signature.
        let create: CreateInstance = unsafe { std::mem::transmute(create) };
        let mut compiler: Option<IDxcCompiler3> = None;
        // SAFETY: the pointer is to an interface of the IID asked for, which
        // `Option<IDxcCompiler3>` holds.
        unsafe { create(&CLSID_DxcCompiler, &IDxcCompiler3::IID, (&raw mut compiler).cast()) }
            .ok()
            .map_err(|error| format!("DxcCreateInstance: {error}"))?;
        let compiler = compiler.ok_or_else(|| "DxcCreateInstance made no compiler".to_owned())?;
        Ok(Dxc { compiler, _library: library })
    }

    /// Compiles a stage's HLSL to DXIL, for shader model 6.0.
    pub(crate) fn compile(&self, stage: Stage, hlsl: &Hlsl) -> Result<Vec<u8>, String> {
        let entry_point: Vec<u16> = hlsl.entry_point.encode_utf16().chain([0]).collect();
        let profile = match stage {
            Stage::Vertex => w!("vs_6_0"),
            Stage::Fragment => w!("ps_6_0"),
        };
        let arguments = [
            w!("-E"),
            PCWSTR(entry_point.as_ptr()),
            w!("-T"),
            profile,
            // naga writes HLSL 2018.
            w!("-HV"),
            w!("2018"),
            DXC_ARG_OPTIMIZATION_LEVEL3,
            w!("-no-warnings"),
        ];
        let source = DxcBuffer { Ptr: hlsl.source.as_ptr().cast(), Size: hlsl.source.len(), Encoding: DXC_CP_UTF8.0 };
        // SAFETY: the buffer and the arguments outlive the call.
        let result: IDxcResult = unsafe { self.compiler.Compile(&raw const source, Some(&arguments), None) }
            .map_err(|error| format!("DXC: {error}"))?;
        // SAFETY: the result is DXC's, and the blobs it hands out live as
        // long as they are held.
        unsafe {
            let status = result.GetStatus().map_err(|error| format!("DXC: {error}"))?;
            if status.is_err() {
                let errors = result.GetErrorBuffer().map_err(|error| format!("DXC: {error}"))?;
                return Err(format!("DXC: {}", String::from_utf8_lossy(bytes(&errors))));
            }
            let object = result.GetResult().map_err(|error| format!("DXC: {error}"))?;
            Ok(bytes(&object).to_vec())
        }
    }
}

/// The bytes of a blob.
///
/// # Safety
///
/// The blob's buffer is valid while it is held.
unsafe fn bytes(blob: &IDxcBlob) -> &[u8] {
    // SAFETY: the buffer is the blob's, `size` bytes long.
    unsafe {
        let (data, size) = (blob.GetBufferPointer(), blob.GetBufferSize());
        if size == 0 { &[] } else { std::slice::from_raw_parts(data.cast(), size) }
    }
}
