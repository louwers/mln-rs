// Ports include/mln/gfx/shader_group.hpp and src/mln/gfx/shader_group.cpp
//
// Deviation: the C++ group stores `shared_ptr<gfx::Shader>` and callers
// `static_pointer_cast` to `ShaderProgramBase`. Every registered shader is a
// program, so the port stores `ShaderProgramBasePtr` (upcast to `dyn Shader`
// where needed). The `shared_mutex` becomes a `RefCell` (one render thread).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::context::Context;
use crate::projection_variant::ProjectionVariant;
use crate::shaders::shader_program_base::ShaderProgramBasePtr;

/// `StringIDSetsPair = pair<unordered_set<string_view>, unordered_set<size_t>>`
pub type StringIDSetsPair = (HashSet<String>, HashSet<usize>);

pub type PropertyHashType = u64;

/// `ShaderGroup::globeDefine`: the define that compiles a shader's globe
/// variant.
pub const GLOBE_DEFINE: &str = "PROJECTION_GLOBE";

/// The data members of `gfx::ShaderGroup`.
#[derive(Default)]
pub struct ShaderGroupData {
    programs: RefCell<HashMap<String, ShaderProgramBasePtr>>,
}

pub trait ShaderGroup {
    fn shader_group_data(&self) -> &ShaderGroupData;

    /// Get a shader from the group by name.
    fn get_shader(&self, shader_name: &str) -> Option<ShaderProgramBasePtr> {
        self.shader_group_data().programs.borrow().get(shader_name).cloned()
    }

    /// Register a new shader under `shader_name`. False on a name conflict.
    fn register_shader_named(&self, shader: ShaderProgramBasePtr, shader_name: &str) -> bool {
        let mut programs = self.shader_group_data().programs.borrow_mut();
        if programs.contains_key(shader_name) {
            return false;
        }
        programs.insert(shader_name.to_owned(), shader);
        true
    }

    /// `get<T>(shaderName)`: the shader if its type name matches.
    fn get_typed(&self, shader_name: &str, type_name: &str) -> Option<ShaderProgramBasePtr> {
        self.get_shader(shader_name).filter(|s| s.type_name() == type_name)
    }

    /// Get a shader by its set of data driven properties as uniforms; if
    /// none is found, create and register it. The base returns none.
    fn get_or_create_shader(
        &self,
        _context: &dyn Context,
        _properties_as_uniforms: &StringIDSetsPair,
        _variant: ProjectionVariant,
        _first_attrib_name: &str,
    ) -> Option<ShaderProgramBasePtr> {
        None
    }
}

pub type ShaderGroupPtr = Rc<dyn ShaderGroup>;

/// `ShaderGroup::getShaderName(name, key)`: `name + '#' + hex(key)`.
#[must_use]
pub fn get_shader_name(name: &str, key: PropertyHashType) -> String {
    format!("{name}#{key:x}")
}

/// `ShaderGroup::propertyHash`: an order-independent hash of the property
/// ids, the wrapping sum of their hashes. Only used as a cache key.
#[must_use]
pub fn property_hash(properties_as_uniforms: &StringIDSetsPair) -> PropertyHashType {
    properties_as_uniforms.1.iter().fold(0u64, |acc, &id| acc.wrapping_add(crate::util_hash::hash(&[id as u64])))
}
