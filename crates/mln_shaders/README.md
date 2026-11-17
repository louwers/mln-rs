# mln-rs-shaders

The renderer's shader programs, each written once in
[WESL](https://wesl-lang.dev) and translated for every backend at run time:

- WESL (`wesl` crate) links a program's module and the shared modules it imports
  into WGSL, with the variant's feature flags deciding its `@if`s.
- The crate rewrites the program's resource declarations into the target
  backend's binding convention, then naga parses and validates the WGSL.
- naga's back ends write SPIR-V (Vulkan), HLSL (D3D12), GLSL ES 3.00 or GLSL
  3.30 (OpenGL, WebGL2) and MSL (Metal); WebGPU takes the WGSL itself.

A backend asks for a program with `program_of(key, target)`, which links each
variant once a process. While programs move over, a backend falls back to its
own sources for a program that has no WESL here; `MLN_SHADERS=native` makes the
backends use their own sources for every program.

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
with the type the WebGPU WGSL gives it: integer inputs (`vec2<i32>` for
`Short2`) read integers on every backend.

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
| Metal   | buffer slot of its id                                        | texture and sampler slot of its id    | bytes at slot 1   |
| WebGPU  | group 0, binding id                                          | next group, as written                | last group, alone |

## Adding a program

1. Write `shaders/programs/<BuiltIn>.wesl`. Start from the program's WebGPU WGSL
   (`mln_webgpu`'s `wgsl_sources.rs`), but keep the hand-written GL and Vulkan
   shaders' design where they read a uniform property in the fragment stage
   rather than passing it as a varying: CPU rasterizers pay for every varying.
   `CircleShader.wesl` is the model.
2. Import what it needs from the shared modules. Don't edit them unless a helper
   is missing.
3. Check each backend's table for the program still describes it: the attributes
   the program reads by id, and, for OpenGL, an entry for each uniform block's
   id (its element size).
4. `cargo test -p mln-rs-shaders` links and validates every variant for every
   target and writes each target's language.
   `T=<target> D=<flags> P=<BuiltIn> cargo test -p mln-rs-shaders print_variant -- --ignored --nocapture`
   prints a variant.
5. Run the program's render tests on Vulkan, OpenGL and WebGPU
   (`MLN_RENDER_BACKEND`), and compare against `MLN_SHADERS=native`.

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
  1, the others' 0 to 1) differs between the hand-written shaders: read what
  they did, and use `@if(gl)` if it must differ.
- A texture that a program only loads texels of (`textureLoad`) is unfilterable
  on WebGPU; sample and load textures through their globals, not function
  parameters, for the crate to tell.
- naga's GLSL has no `f16` and writes `highp`; use `f32`.
- An alpha-only texture (`A8Unorm`: the glyph atlas, line dash patterns) reads
  its value from alpha on every backend but WebGPU, which has no alpha-only
  format and reads it from red: use `@if(webgpu)`.
- Metal's line shaders evaluate GPU expressions (`Caps::line_expressions`),
  whose expression mask means the opposite of the other backends'. A WESL line
  program needs Metal's caps to match it.
