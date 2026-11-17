# mln-rs-shaders

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because the
> other mln-rs crates depend on it, and its API can change in any release
> without notice. mln-rs doesn't re-export it.

The renderer's shader programs, each written once in
[WESL](https://wesl-lang.dev) and translated for every backend at run time:

- WESL (`wesl` crate) links a program's module and the shared modules it imports
  into WGSL, with the variant's feature flags deciding its `@if`s.
- The crate rewrites the program's resource declarations into the target
  backend's binding convention, then naga parses and validates the WGSL.
- naga's back ends write SPIR-V (Vulkan), HLSL (D3D12), GLSL ES 3.00 or GLSL
  3.30 (OpenGL, WebGL2) and MSL (Metal); WebGPU takes the WGSL itself.

A backend asks for a program with `program_of(key, target)`, which links each
variant once a process and reports the program's resources and vertex inputs,
from which the backend binds it.

## Modules

- `shaders/programs/<BuiltIn>.wesl`: a program, named after its `BuiltIn`
  variant. The build script registers every file there; a misnamed one doesn't
  compile.
- `shaders/common.wesl`: constants, GLSL's `mod`, the unpacking of paint
  properties packed into attributes, pattern and atlas positions.
- `shaders/globals.wesl`: the global uniforms (`paintParams`) and
  `drawable_index()`, the index of the draw's element in its layer's
  per-drawable buffers.
- `shaders/projection.wesl`: `ProjectionUBO`, the per-drawable projection buffer
  (`load_projection()`), and the Mercator and globe projections.

A module of helpers that several programs of one layer share goes beside them in
`shaders/`, owned by that layer's programs.

## Conventions

Entry points are `vs_main` and `fs_main`.

Feature flags are the `ShaderKey`'s defines that have no value
(`HAS_UNIFORM_u_<name>`, `PROJECTION_GLOBE`, `OVERDRAW_INSPECTOR`) and the
target's own: `vulkan`, `d3d12`, `gl`, `metal` or `webgpu`. A define with a
value, such as `DEVICE_PIXEL_RATIO`, is not a flag: read the value from a
uniform (`paintParams.pixel_ratio`).

A vertex input's `@location` is its attribute's id (`ID_<LAYER>_..._ATTRIBUTE`),
and its type the attribute's: integer inputs (`vec2<i32>` for `Short2`) read
integers on every backend. A backend binds only the attributes a variant reads.

A resource's `@group` is its descriptor set, and its `@binding` the renderer's
id (`mln_gfx::shaders::shader_defines`):

| `@group`                          | holds                       | `@binding`                 |
| --------------------------------- | --------------------------- | -------------------------- |
| 0: global                         | `paintParams`               | the buffer's id            |
| 1: the layer's                    | per-drawable arrays, props  | the buffer's id            |
| 2: the drawable's uniform buffers | uniform buffers of one draw | the buffer's id            |
| 3: the drawable's images          | textures, samplers          | texture id; 3 + texture id |

A layer's buffer with an element for each drawable is a `var<storage, read>`
array, indexed with `drawable_index()`; a layer's evaluated properties are a
`var<uniform>` in group 1. Each backend links the declarations rewritten into
its own convention:

| Backend | Buffers                                                      | Textures and samplers                 | Draw index        |
| ------- | ------------------------------------------------------------ | ------------------------------------- | ----------------- |
| Vulkan  | the `layout` set and binding                                 | image set, texture id; 3 + id         | push constant     |
| D3D12   | as Vulkan, then space = set, register = binding              | as Vulkan, samplers by index heap     | root constant     |
| OpenGL  | block at the binding point of its id; arrays are one element | sampler at the texture unit of its id | 0                 |
| Metal   | buffer slot of its id; vertex buffers after the last         | texture and sampler slot of its id    | bytes at slot 1   |
| WebGPU  | group 0, binding id                                          | next group, as written                | last group, alone |

## Adding a program

1. Write `shaders/programs/<BuiltIn>.wesl`. Read a uniform property in the
   fragment stage rather than passing it as a varying: CPU rasterizers pay for
   every varying. `CircleShader.wesl` is the model.
2. Import what it needs from the shared modules. Don't edit them unless a helper
   is missing.
3. `cargo test -p mln-rs-shaders` links and validates every variant for every
   target, writes each target's language, and checks each buffer against the
   renderer's UBO of its name (`tests.rs`, which lists them).
   `T=<target> D=<flags> P=<BuiltIn> cargo test -p mln-rs-shaders print_variant -- --ignored --nocapture`
   prints a variant.
4. Each backend's `every_program_compiles_for_gpu` test (`every_program_links`
   on OpenGL) makes the program's variants on its device.
5. Run the program's render tests on Vulkan, OpenGL and WebGPU
   (`MLN_RENDER_BACKEND`).

## Pitfalls

- WGSL's uniformity rules apply: `textureSample` needs uniform control flow,
  where GLSL sampled anywhere. Sample before branching, or use
  `textureSampleLevel`.
- `@if` on a compound statement inlines it into the enclosing scope, so a `let`
  in it is visible after it; `@if` works on declarations, struct members and
  statements, and `@elif`/`@else` follow it.
- An `@if(PROJECTION_GLOBE) import` brings in globe-only helpers.
- OpenGL, WebGPU, Metal and D3D12 take the clip space the program writes;
  Vulkan's flips Y. A constant that depends on the depth range (OpenGL's -1 to
  1, the others' 0 to 1) takes `@if(gl)`.
- On OpenGL, `@builtin(position)` in the fragment stage starts at the window's
  bottom left (`gl_FragCoord`), and at its top left elsewhere: a program that
  compares it with a position from the vertex stage converts it `@if(gl)`
  (`fill::fragment_position`).
- On OpenGL, a texture that an earlier pass drew into starts at its bottom row:
  a program that samples one flips v `@if(gl)` (`HeatmapTextureShader`,
  `HillshadeShader`).
- A texture that a program only loads texels of (`textureLoad`) is unfilterable
  on WebGPU; sample and load textures through their globals, not function
  parameters, for the crate to tell.
- naga's GLSL has no `f16` and writes `highp`; use `f32`.
- An alpha-only texture (`A8Unorm`: the glyph atlas, line dash patterns) reads
  its value from alpha on every backend but WebGPU, which has no alpha-only
  format and reads it from red: use `@if(webgpu)`.
- A program picks a property's value by its `HAS_UNIFORM_u_<name>` flag, not by
  an expression mask such as `LineEvaluatedPropsUBO`'s.
- A layer's per-drawable buffer holds elements of the size of its layer's
  largest UBO, such as `LineDrawableUnionUBO`: pad the program's struct to it.
- A program can import another's module: the per-vertex fill extrusion walls
  wrap the instanced walls' stages.
