// Ports test/renderer/shader_registry.test.cpp (TEST(ShaderRegistry, *)).
//
// Not ported: the DISABLED_GLSLReplacement* tests (OpenGL legacy programs,
// disabled in C++ too). The `MapInstance` helper they use is unused here.
//
// Deviation: shader groups store `ShaderProgramBase`s (see shader_group.rs),
// so the stub programs implement it trivially.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use mln_gfx::shader::Shader;
use mln_gfx::shader_group::ShaderGroup;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBase, ShaderProgramBasePtr};
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_util::identity::SimpleIdentity;

/// `StubProgramBase<token_value>` + `StubProgram_1` / `StubProgram_2`
struct StubProgram {
    name: &'static str,
    token: Cell<u32>,
    id: SimpleIdentity,
    attrs: VertexAttributeArray,
}

impl StubProgram {
    fn new(name: &'static str, token: u32) -> Self {
        StubProgram { name, token: Cell::new(token), id: SimpleIdentity::new(), attrs: VertexAttributeArray::new() }
    }
    fn draw(&self) -> u32 {
        self.token.get()
    }
    fn set_token(&self, tok: u32) {
        self.token.set(tok);
    }
}

const STUB_PROGRAM_1: &str = "StubProgram_1";
const STUB_PROGRAM_2: &str = "StubProgram_2";

fn stub_program_1() -> Rc<StubProgram> {
    Rc::new(StubProgram::new(STUB_PROGRAM_1, 10))
}
fn stub_program_2() -> Rc<StubProgram> {
    Rc::new(StubProgram::new(STUB_PROGRAM_2, 20))
}

impl Shader for StubProgram {
    fn type_name(&self) -> &'static str {
        self.name
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl ShaderProgramBase for StubProgram {
    fn get_id(&self) -> SimpleIdentity {
        self.id
    }
    fn get_sampler_location(&self, _id: usize) -> Option<usize> {
        None
    }
    fn get_vertex_attributes(&self) -> &VertexAttributeArray {
        &self.attrs
    }
    fn get_instance_attributes(&self) -> &VertexAttributeArray {
        &self.attrs
    }
}

/// `shader->to<T>()`
fn to_stub<'a>(shader: &'a dyn Shader, type_name: &str) -> Option<&'a StubProgram> {
    if shader.type_name() != type_name {
        return None;
    }
    shader.as_any().downcast_ref::<StubProgram>()
}

/// `StubShaderConsumer::useShader<T>(registry)`
fn use_shader(registry: &ShaderRegistry, type_name: &str) -> u32 {
    let program = registry.get_legacy_group().get_typed(type_name, type_name);
    program.as_deref().and_then(|p| to_stub(p, type_name)).map_or(0, StubProgram::draw)
}

fn same(a: Option<&ShaderProgramBasePtr>, b: &ShaderProgramBasePtr) -> bool {
    a.is_some_and(|a| std::ptr::addr_eq(Rc::as_ptr(a), Rc::as_ptr(b)))
}

/// Ensure we can register a `gfx::Shader-based` type with a registry object
// gtest: ShaderRegistry.RegisterShader
#[test]
fn register_shader() {
    let registry = ShaderRegistry::new();
    // The registry should start empty
    assert!(!registry.get_legacy_group().is_shader(STUB_PROGRAM_1));

    // Register the program
    let program: ShaderProgramBasePtr = stub_program_1();
    assert!(registry.get_legacy_group().register_shader(program.clone()));
    // We can't re-register the same program name
    assert!(!registry.get_legacy_group().register_shader(program.clone()));
    // Ensure the shader is present in the registry now
    assert!(registry.get_legacy_group().is_shader(STUB_PROGRAM_1));
    // And we can fetch it
    assert!(same(registry.get_legacy_group().get_typed(STUB_PROGRAM_1, STUB_PROGRAM_1).as_ref(), &program));

    // Make sure downcasting to program1 works as expected
    assert_eq!(use_shader(&registry, STUB_PROGRAM_1), 10);
}

/// Validate replacing a shader by name with a different instance
// gtest: ShaderRegistry.ReplaceShaderType
#[test]
fn replace_shader_type() {
    let registry = ShaderRegistry::new();
    assert!(!registry.get_legacy_group().is_shader(STUB_PROGRAM_1));

    let program: ShaderProgramBasePtr = stub_program_1();
    assert!(registry.get_legacy_group().register_shader(program.clone()));
    // Ensure the shader is present in the registry now
    assert!(registry.get_legacy_group().is_shader(STUB_PROGRAM_1));
    assert!(registry.get_legacy_group().is_shader(program.type_name()));

    // Make sure downcasting to program1 works as expected
    assert_eq!(use_shader(&registry, STUB_PROGRAM_1), 10);

    // StubProgram_1 and StubProgram_2 are disconnected types, and as such
    // this replacement should fail as StubProgram_2 has never been registered.
    let program2: ShaderProgramBasePtr = stub_program_2();
    assert!(!registry.get_legacy_group().replace_shader(program2));

    // Make a second instance of program1 and change the token
    let program3 = stub_program_1();
    program3.set_token(30);
    // Replace it in the registry
    assert!(registry.get_legacy_group().replace_shader(program3));

    // Assert the new program downcasts from the registry as expected
    assert_eq!(use_shader(&registry, STUB_PROGRAM_1), 30);
}

/// Validate shader type info and downcasting are functioning
// gtest: ShaderRegistry.ShaderRTTI
#[test]
fn shader_rtti() {
    let program1 = StubProgram::new(STUB_PROGRAM_1, 10);
    let as_base: &dyn Shader = &program1;
    // Should convert to program 1 but not program 2
    assert!(to_stub(as_base, STUB_PROGRAM_1).is_some_and(|p| std::ptr::eq(p, &raw const program1)));
    assert!(to_stub(as_base, STUB_PROGRAM_2).is_none());
}

/// Register the same type under different names
// gtest: ShaderRegistry.MultiRegister
#[test]
fn multi_register() {
    let registry = ShaderRegistry::new();
    let group = registry.get_legacy_group();
    assert!(group.register_shader(stub_program_1()));
    assert!(group.register_shader_named(stub_program_1(), "SecondProgram"));

    // Default option, register as the type name
    let a = group.get_typed(STUB_PROGRAM_1, STUB_PROGRAM_1);
    assert!(a.is_some());
    // Register with an explicit name
    let b = group.get_typed("SecondProgram", STUB_PROGRAM_1);
    assert!(b.is_some());
    assert!(!same(a.as_ref(), b.as_ref().unwrap()));
}

/// `populate(to, name)`: fill `to` when empty.
fn populate(group: &dyn ShaderGroup, to: &mut Option<ShaderProgramBasePtr>, name: &str, type_name: &str) -> bool {
    if to.is_some() {
        return true;
    }
    *to = group.get_typed(name, type_name);
    to.is_some()
}

/// Test fetching
// gtest: ShaderRegistry.RegistryFetch
#[test]
fn registry_fetch() {
    let registry = ShaderRegistry::new();
    let group = registry.get_legacy_group();
    assert!(group.register_shader(stub_program_1()));
    assert!(group.register_shader_named(stub_program_1(), "SecondProgram"));

    let mut prog_a = None;
    let mut prog_b = None;
    assert!(populate(group, &mut prog_a, STUB_PROGRAM_1, STUB_PROGRAM_1));
    assert!(populate(group, &mut prog_b, "SecondProgram", STUB_PROGRAM_1));
    assert!(!same(prog_a.as_ref(), prog_b.as_ref().unwrap()));
    assert!(prog_a.is_some());
    assert!(prog_b.is_some());
}

/// Replace a manually named shader
// gtest: ShaderRegistry.NamedReplace
#[test]
fn named_replace() {
    let registry = ShaderRegistry::new();
    let group = registry.get_legacy_group();

    // Register
    assert!(group.register_shader_named(stub_program_1(), "CustomName"));
    let mut prog_a = None;
    assert!(populate(group, &mut prog_a, "CustomName", STUB_PROGRAM_1));
    assert!(prog_a.is_some());

    // Replace it with a new instance
    assert!(group.replace_shader_named(stub_program_1(), "CustomName"));
    let mut prog_b = None;
    assert!(populate(group, &mut prog_b, "CustomName", STUB_PROGRAM_1));
    assert!(prog_b.is_some());

    // Should be different instances
    assert!(!same(prog_a.as_ref(), prog_b.as_ref().unwrap()));
}

// The three GLSL replacement tests are OpenGL-only in C++
// (`#ifdef MLN_RENDER_BACKEND_OPENGL`) and disabled there (`DISABLED_`
// prefix): they replace the legacy GL ClippingMaskProgram with GLSL sources,
// render test/fixtures/api/water.json and compare with
// test/fixtures/shader_registry/glsl_replace_*. The port's shaders are WGSL
// on wgpu.

/// Test replacing an actual program instance with a similar instance
// gtest: ShaderRegistry.DISABLED_GLSLReplacement_NoOp
#[test]
#[ignore = "disabled in C++ (DISABLED_) and OpenGL-only (replaces the legacy GL ClippingMaskProgram); the port's shaders are WGSL on wgpu"]
fn disabled_glsl_replacement_no_op() {
    // test/renderer/shader_registry.test.cpp:214
}

/// Test replacing an actual program with a similar instance using a
/// different fragment shader
// gtest: ShaderRegistry.DISABLED_GLSLReplacement1
#[test]
#[ignore = "disabled in C++ (DISABLED_) and OpenGL-only (GLSL fragment source for the legacy GL ClippingMaskProgram); the port's shaders are WGSL on wgpu"]
fn disabled_glsl_replacement1() {
    // test/renderer/shader_registry.test.cpp:241
}

/// Test replacing an actual program with a similar instance using a
/// different fragment shader
// gtest: ShaderRegistry.DISABLED_GLSLReplacement2
#[test]
#[ignore = "disabled in C++ (DISABLED_) and OpenGL-only (GLSL fragment source for the legacy GL ClippingMaskProgram); the port's shaders are WGSL on wgpu"]
fn disabled_glsl_replacement2() {
    // test/renderer/shader_registry.test.cpp:275
}
