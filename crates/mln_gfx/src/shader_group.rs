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

    /// Checks if a shader exists in the group for the given name.
    fn is_shader(&self, shader_name: &str) -> bool {
        self.shader_group_data().programs.borrow().contains_key(shader_name)
    }

    /// Get a shader from the group by name.
    fn get_shader(&self, shader_name: &str) -> Option<ShaderProgramBasePtr> {
        self.shader_group_data().programs.borrow().get(shader_name).cloned()
    }

    /// Replace a matching shader (by its type name). False if none matched.
    fn replace_shader(&self, shader: ShaderProgramBasePtr) -> bool {
        let name = shader.type_name().to_owned();
        self.replace_shader_named(shader, &name)
    }

    fn replace_shader_named(&self, shader: ShaderProgramBasePtr, shader_name: &str) -> bool {
        let mut programs = self.shader_group_data().programs.borrow_mut();
        if !programs.contains_key(shader_name) {
            return false;
        }
        programs.insert(shader_name.to_owned(), shader);
        true
    }

    /// Register a new shader (by its type name). False on a name conflict.
    fn register_shader(&self, shader: ShaderProgramBasePtr) -> bool {
        let name = shader.type_name().to_owned();
        self.register_shader_named(shader, &name)
    }

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
/// ids (`util::order_independent_hash`).
// TODO(port): the C++ combination (sum of std::hash values) is approximated;
// only used as a cache key.
#[must_use]
pub fn property_hash(properties_as_uniforms: &StringIDSetsPair) -> PropertyHashType {
    properties_as_uniforms.1.iter().fold(0u64, |acc, &id| acc.wrapping_add(crate::util_hash::hash(&[id as u64])))
}

/// A plain `gfx::ShaderGroup` (the registry's legacy group).
#[derive(Default)]
pub struct BasicShaderGroup {
    data: ShaderGroupData,
}

impl ShaderGroup for BasicShaderGroup {
    fn shader_group_data(&self) -> &ShaderGroupData {
        &self.data
    }
}
