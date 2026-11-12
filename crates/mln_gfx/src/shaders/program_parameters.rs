// Ports include/mln/shaders/program_parameters.hpp and
// src/mln/shaders/program_parameters.cpp

use std::cell::OnceCell;

use mln_vec_map::VecMap;

use crate::backend::Type as BackendType;
use crate::shaders::shader_source::BuiltIn;

const BACKEND_TYPE_COUNT: usize = 4;

#[derive(Clone, Debug, Default)]
pub struct ProgramSource {
    pub backend: Option<BackendType>,
    pub vertex: String,
    pub fragment: String,
}

impl ProgramSource {
    /// The provided source code is intended for use with the given backend.
    #[must_use]
    pub fn new(for_backend: BackendType, vertex: &str, fragment: &str) -> Self {
        ProgramSource { backend: Some(for_backend), vertex: vertex.to_owned(), fragment: fragment.to_owned() }
    }
}

#[derive(Clone, Debug)]
pub struct ProgramParameters {
    // C++ uses an unordered_map; the order only affects the defines string
    // and its hash (a cache key), so a VecMap keeps it deterministic.
    defines: VecMap<String, String>,
    defines_hash: u64,
    program_type: BuiltIn,
    defines_str: OnceCell<String>,
    default_sources: [ProgramSource; BACKEND_TYPE_COUNT],
    user_sources: [ProgramSource; BACKEND_TYPE_COUNT],
    overdraw_inspector: bool,
}

/// `util::toString(float, /*decimal=*/true)` for the define value: rapidjson's
/// shortest double formatting, keeping the trailing ".0".
// TODO(port): use the rapidjson dtoa port once available; the value is only
// used inside the defines string (cache key), never in shader output.
fn to_string_decimal(value: f32) -> String {
    let v = f64::from(value);
    if v.is_finite() && v == v.trunc() { format!("{v:.1}") } else { format!("{v}") }
}

impl ProgramParameters {
    #[must_use]
    pub fn new(pixel_ratio: f32, overdraw: bool) -> Self {
        let mut defines = VecMap::new();
        defines.insert("DEVICE_PIXEL_RATIO".to_owned(), to_string_decimal(pixel_ratio));
        if overdraw {
            defines.insert("OVERDRAW_INSPECTOR".to_owned(), String::new());
        }
        let mut params = ProgramParameters {
            defines,
            defines_hash: 0,
            program_type: BuiltIn::None,
            defines_str: OnceCell::new(),
            default_sources: Default::default(),
            user_sources: Default::default(),
            overdraw_inspector: overdraw,
        };
        params.defines_hash = crate::util_hash::hash_str(params.get_defines_string());
        params
    }

    /// Provide custom shader code which overrides any default source present
    #[must_use]
    pub fn with_shader_source(&self, source: &ProgramSource) -> Self {
        let mut params = self.clone();
        let backend = source.backend.expect("ProgramSource without backend");
        params.user_sources[backend as usize] = source.clone();
        params
    }

    /// Provide default shader source
    #[must_use]
    pub fn with_default_source(&self, source: &ProgramSource) -> Self {
        let mut params = self.clone();
        let backend = source.backend.expect("ProgramSource without backend");
        params.default_sources[backend as usize] = source.clone();
        params
    }

    /// Provide a `BuiltIn` value used to indicate the program type
    #[must_use]
    pub fn with_program_type(&self, type_: BuiltIn) -> Self {
        let mut params = self.clone();
        params.program_type = type_;
        params
    }

    pub fn get_program_type(&self) -> BuiltIn {
        self.program_type
    }

    /// Built-in shader preprocessor defines as `#define KEY VALUE` lines.
    pub fn get_defines_string(&self) -> &str {
        self.defines_str.get_or_init(|| {
            let mut s = String::with_capacity(self.defines.len() * 32);
            for (key, value) in &self.defines {
                s.push_str("#define ");
                s.push_str(key);
                if !value.is_empty() {
                    s.push(' ');
                    s.push_str(value);
                }
                s.push('\n');
            }
            s
        })
    }

    pub fn get_defines(&self) -> &VecMap<String, String> {
        &self.defines
    }

    /// Errors with the C++ exception message when no source is available.
    pub fn vertex_source(&self, backend: BackendType) -> Result<&str, String> {
        let i = backend as usize;
        if !self.user_sources[i].vertex.is_empty() {
            Ok(&self.user_sources[i].vertex)
        } else if !self.default_sources[i].vertex.is_empty() {
            Ok(&self.default_sources[i].vertex)
        } else {
            Err("No vertex shader source provided for selected backend!".to_owned())
        }
    }

    pub fn fragment_source(&self, backend: BackendType) -> Result<&str, String> {
        let i = backend as usize;
        if !self.user_sources[i].fragment.is_empty() {
            Ok(&self.user_sources[i].fragment)
        } else if !self.default_sources[i].fragment.is_empty() {
            Ok(&self.default_sources[i].fragment)
        } else {
            Err("No fragment shader source provided for selected backend!".to_owned())
        }
    }

    pub fn get_overdraw_inspector_enabled(&self) -> bool {
        self.overdraw_inspector
    }

    pub fn get_defines_hash(&self) -> u64 {
        self.defines_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defines() {
        let p = ProgramParameters::new(1.0, false);
        assert_eq!(p.get_defines_string(), "#define DEVICE_PIXEL_RATIO 1.0\n");
        let p = ProgramParameters::new(2.0, true);
        assert!(p.get_defines().contains_key("OVERDRAW_INSPECTOR"));
        assert!(p.vertex_source(BackendType::WebGPU).is_err());
    }
}
