// Ports include/mln/gfx/shader_registry.hpp and src/mln/gfx/shader_registry.cpp

use std::cell::RefCell;
use std::collections::HashMap;

use crate::shader_group::{BasicShaderGroup, ShaderGroupPtr};

/// A collection of `ShaderGroup`s, registered or replaced by name at run
/// time. `&self` methods: the C++ `shared_mutex` becomes a `RefCell`.
#[derive(Default)]
pub struct ShaderRegistry {
    legacy_group: BasicShaderGroup,
    shader_groups: RefCell<HashMap<String, ShaderGroupPtr>>,
}

impl ShaderRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the legacy shader group.
    pub fn get_legacy_group(&self) -> &BasicShaderGroup {
        &self.legacy_group
    }

    /// Checks if a shader group exists in the registry for the given name.
    pub fn is_shader_group(&self, shader_group_name: &str) -> bool {
        self.shader_groups.borrow().contains_key(shader_group_name)
    }

    /// Get a shader group from the registry by name.
    pub fn get_shader_group(&self, shader_group_name: &str) -> Option<ShaderGroupPtr> {
        self.shader_groups.borrow().get(shader_group_name).cloned()
    }

    /// Replace a matching shader group. False if no match was found.
    pub fn replace_shader(&self, shader: ShaderGroupPtr, shader_group_name: &str) -> bool {
        let mut groups = self.shader_groups.borrow_mut();
        if !groups.contains_key(shader_group_name) {
            return false;
        }
        groups.insert(shader_group_name.to_owned(), shader);
        true
    }

    /// Register a new shader group. False if the name is already taken.
    pub fn register_shader_group(&self, shader_group: ShaderGroupPtr, shader_group_name: &str) -> bool {
        let mut groups = self.shader_groups.borrow_mut();
        if groups.contains_key(shader_group_name) {
            return false;
        }
        groups.insert(shader_group_name.to_owned(), shader_group);
        true
    }
}
