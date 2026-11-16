# mln-rs-c

The MapLibre Native C API: the headers in `include/` define it, and this crate
implements it as the C library `libmaplibre_native_c`.

[maplibre-native-ffi](https://github.com/maplibre/maplibre-native-ffi) builds
this crate from a pinned mln-rs commit, packages the library for each platform,
and generates bindings for other languages from the headers. Its copy of the
headers must match the ones here, so it takes theirs from the commit it pins.

## Changing the API

- After editing a header, run `mise run generate:c-abi`. `src/lib.rs` explains
  what it generates and how to implement a function.
- Test what the API adds in this crate's tests. `mise run test:c-abi` also runs
  maplibre-native-ffi's ABI suite, which `mise run update:ffi-abi` copies here,
  against the library.
- Follow the guidelines below when you design or review a change to the headers.

## API guidelines

If a rule doesn't fit a case, change the rule in the same PR instead of making
an exception. When a tool enforces a rule, delete it here.

### Compatibility

`MLN_C_VERSION` and `mln_c_version()` are the API's epoch. In epoch 0, nothing
stays compatible: make breaking changes when they improve the API, and don't add
compatibility shims. In epoch 1 and later, a release doesn't break the API, ABI,
or documented behavior of earlier releases with the same epoch. A breaking
change increments the epoch.

### Annotations

Besides the C types, the headers carry `MLN_BINDING` annotations. They say who
owns each value, how long it lives, whether it can be null, and how each
function runs. maplibre-native-ffi's generator uses them to write safe wrappers
in each binding.

### Naming

- Name a function `mln_<owner>_<verb>[_<noun>]`, where the owner is the type of
  its first parameter: `mln_map_get_style_layer` takes an `mln_map`. A function
  without a handle parameter starts with its subsystem instead:
  `mln_log_set_callback`.
- Use `get_` for a function that reads one value, `list_` for one that reads all
  of them, and `is_` for a predicate.
- Name a bitmask enum `mln_<x>_flag`, and the type that holds a set of its bits
  `mln_<x>_flags`.
- Prefer the MapLibre style spec's terms over engine terms.
- Camera terms from the style spec keep their spec names and units: `bearing`,
  `pitch`, `roll`, and `field_of_view` are in degrees, and `center_altitude` is
  in meters. Their comments state the unit.
- A `_size` is a count of bytes, and a `_count` is a count of items.
- When another plain number's type doesn't show its unit, end its name with the
  unit: `duration_ms`, `expires_unix_ms`.

### General API design

- A function that returns an error status must not change any state, except an
  answer to a request, such as `mln_resource_request_complete`. An answer ends
  its handle whatever it returns, and carries `MLN_BINDING("consumes=always")`.
- An asynchronous function takes an `mln_completion`, which receives its result.
- Annotate each function with how it runs, as in
  `MLN_BINDING("execution=command")`:

  | Execution     | Function                                                   |
  | ------------- | ---------------------------------------------------------- |
  | `command`     | Changes state asynchronously                               |
  | `query`       | Reads state asynchronously                                 |
  | `operation`   | Does other asynchronous work                               |
  | `lifecycle`   | Creates, attaches, or retires a handle asynchronously      |
  | `immediate`   | Returns its result synchronously; the default              |
  | `snapshot`    | Copies the current state behind a handle synchronously     |
  | `event_batch` | Takes the queued frame results as a batch, or a null batch |

- Fix surprising behavior instead of documenting it. When a contract is hard to
  state, change the API instead of writing more prose.

### Handles

A handle is an opaque 64-bit ID for state that the library owns and keeps
between calls, such as a runtime, a map, a parsed style expression, or a
delivery queue. Callers can copy and compare handles and pass them between
threads, but can't see the state behind a handle, and must not create or decode
one. `MLN_HANDLE_NULL` is the null handle. The library never reuses a released
handle.

```c
typedef uint64_t mln_style_expression MLN_BINDING(
  "kind=handle;release=mln_style_expression_release"
);
typedef uint64_t mln_map MLN_BINDING(
  "kind=handle;release=mln_map_release;dispose=mln_map_dispose"
);

// A create function writes the handle to an output that must hold the null
// handle on entry.
mln_style_expression expression = MLN_HANDLE_NULL;
mln_style_expression_create(json, NULL, &expression, NULL);
mln_style_expression_release(expression);
```

Most handles are freed right away, so they have one teardown function,
`void _release(handle)`. Some handles can't be freed until other work finishes,
such as a runtime that has to wait for its maps to retire. These have two:

- `_release(handle, completion)` is for an orderly shutdown. It rejects a handle
  that isn't ready, such as a runtime that still has maps, and the completion
  runs once teardown finishes.
- `_dispose(handle)` is for destructors and finalizers, which can't wait or
  handle errors. It accepts the handle in any state and finishes teardown in the
  background.

Bindings name these `release` and `dispose` in their own case, such as
`ReleaseAsync` and `Dispose` in .NET, and have no other name for either.

### Types that may grow

In epoch 1 and later, a release can add to the API but can't break existing
declarations. Code built against older headers must keep working with a newer
library, so each type is declared in a way that lets later releases extend it.

```c
typedef enum mln_status : int32_t {
  MLN_STATUS_OK = 0,
  MLN_STATUS_INVALID_ARGUMENT = -1,
  MLN_STATUS_INVALID_STATE = -2,
  // A later release adds -3, -4, and so on.
} mln_status;

typedef struct mln_diagnostic {
  uint32_t size; // sizeof(mln_diagnostic) in the caller's headers
  char message[MLN_DIAGNOSTIC_MESSAGE_CAPACITY];
  // A later release adds members here.
} mln_diagnostic;

typedef struct mln_lat_lng { // No size: this type never changes.
  double latitude;
  double longitude;
} mln_lat_lng;
```

- Use fixed-width integers, `size_t`, `float`, `double`, and `bool`.
- An enum declares a fixed underlying type, usually `uint32_t`. A later release
  can add values, so code that reads an enum from the library must accept values
  its headers don't name.
- A struct passed by pointer begins with `uint32_t size`, and a later release
  adds members only at its end. The side that allocates the struct sets `size`
  to its `sizeof`, and the other side reads or writes only the members within
  that size.
- A struct that is only ever embedded by value in another struct has no `size`.
  Its container's `size` versions it.
- A struct that is also passed by pointer on its own keeps its `size` when
  another struct embeds it, as `mln_camera_update` embeds `mln_camera_options`.
  The library ignores the embedded `size`.
- A small value type that is not intended to grow, such as a coordinate or an
  ID, does not need `size`.

### Option bags

An options struct holds a function's settings. Each optional setting has a bit
in the struct's `fields` member. The library reads a setting only when its bit
is set. For a struct that configures one call, the library uses the setting's
default otherwise, so a struct with only `size` set selects every default.

```c
typedef enum mln_style_expression_option_field : uint32_t {
  MLN_STYLE_EXPRESSION_OPTION_EXPECTED_TYPE = 1U << 0U,
} mln_style_expression_option_field;

typedef struct mln_style_expression_options {
  uint32_t size;
  mln_style_expression_option_fields fields;
  mln_style_expression_type expected_type
    MLN_BINDING("mask=fields;bit=MLN_STYLE_EXPRESSION_OPTION_EXPECTED_TYPE");
} mln_style_expression_options;

mln_style_expression_options options = {.size = sizeof(options)};
options.fields = MLN_STYLE_EXPRESSION_OPTION_EXPECTED_TYPE;
options.expected_type.kind = MLN_STYLE_EXPRESSION_TYPE_NUMBER;
```

- The bits are a bitmask enum named `mln_<x>_field`, and `fields` has the type
  `mln_<x>_fields`.
- A member with no sensible default, such as a width or a Vulkan device, is
  required instead. It has no bit and carries `MLN_BINDING("required=true")`.
- A struct that updates state a handle keeps, such as the one
  `mln_map_set_viewport_options` takes, changes only the settings whose bits are
  set. The others keep their current values instead of returning to defaults,
  and the function's comment says so.
- A function that reads back the settings of such a struct reports a record that
  holds every setting, without `size` or `fields`. The record is named for what
  it holds, without `_options`: `mln_camera` for `mln_camera_options`. A
  function that writes the record through a pointer wraps it in a struct that
  begins with `size`, as `mln_map_projection_camera_result` wraps an
  `mln_camera`.

### Data in and out

Neither side frees memory that the other allocated. Data passed into a function
is borrowed for the call. Data that the library returns is either written into
storage that the caller provides, or lent by the library until a stated point,
such as when a callback returns.

```c
// Borrowed for the call: the caller can free json once it returns.
mln_style_expression_create(json, NULL, &expression, NULL);

// Written into the caller's buffer. The first call reports the length.
size_t length = 0;
mln_style_expression_evaluate(expression, NULL, NULL, 0, &length, NULL);
char* result = malloc(length + 1);
mln_style_expression_evaluate(
  expression, NULL, result, length + 1, &length, NULL
);

// Lent until the callback returns, so the caller copies what it keeps.
void on_complete(void* user_data, const mln_completion_result* result) {
  keep_copy(result->diagnostic);
}
```

- Pass text as UTF-8 in an `mln_buffer_view`, not as a NUL-terminated string.
- A function that keeps an input copies it before it returns.
- A synchronous function that returns data of a length the caller can't know
  writes it into a buffer the caller passes, and always reports the full length,
  so the caller can call again with enough room.
- Lent data stays valid until a point that the declaration states, such as when
  a callback returns or a handle is released.

### Callbacks

The library calls caller code through callbacks, to deliver a result, report an
event, or ask the caller for data. A callback comes in a registration struct,
which pairs it with `user_data`, the caller's context pointer, and with a
function that frees that context. The library runs the callback on a thread that
it picks, unless the registration names an `mln_delivery_queue`. Then the
library queues each call, and the callback runs when the caller dispatches the
queue, on a thread the caller chooses.

```c
typedef struct mln_log_handler {
  uint32_t size;
  mln_log_callback callback;
  void* user_data;
  mln_user_data_release release_user_data;
  mln_delivery_queue queue;
  mln_log_severity_flags platform_severities;
} mln_log_handler;

mln_status mln_log_set_callback(
  const mln_log_handler* handler, mln_diagnostic* out_diagnostic
);
```

- The library calls `release_user_data` once, after the last call to the
  callback.
- A registration struct stays a struct even when it holds a single callback, so
  it can gain members without breaking callers.
- A callback accepts a delivery queue unless the library needs the callback's
  result before it can continue.

### Platform-specific APIs

A function that only some platforms can use lists those platforms in its
annotation, and so does a tagged union's variant. A function that integrates
with one platform's own objects, such as Android's Java VM, goes in that
platform's header under `include/maplibre_native_c/platform/`. A feature that
each platform provides in its own way, such as pacing frames to the display, is
one function for every platform, which returns `MLN_STATUS_UNSUPPORTED` where
the platform lacks it.

```c
MLN_BINDING("platforms=android")
MLN_API mln_status mln_android_init(/* ... */);

typedef union mln_render_target_data {
  mln_metal_layer_target metal_layer MLN_BINDING(
    "variant=MLN_RENDER_TARGET_KIND_METAL_LAYER;"
    "platforms=ios,macos,tvos,visionos"
  );
  /* ... */
} mln_render_target_data;
```

A union that describes something of many kinds, such as a render target, has one
variant for each kind, in a struct that holds the kind and the union. Bindings
present the struct as a sum type. A later release adds kinds as new variants.

### Reference comments

The header comments are the API reference, which maplibre-native-ffi's generator
copies into every binding. Write them in plain English for someone integrating
the library: what a declaration does, and what a caller must know to use it
correctly. Promise only that. Behavior that holds today but that an integration
doesn't need stays out of the comment, so we can change it.

A comment has a one-sentence summary, then paragraphs about behavior, then these
sections in order. Leave out a section that would be empty:

- `Threads:` says where a callback runs, when its caller needs to know.
- `Returns:` lists the statuses a correct caller handles, each with its
  condition.
- `Completes with:` does the same for the completion.
- `Lifetimes:` says how long each pointer stays valid and who ends it.

Give every function and type a summary. Comment a field or an enum constant only
when its name and type don't say what a caller needs.

Name parameters in backticks. Leave out what bindings take over: completions,
`out_diagnostic`, output pointers, `size` fields, and a callback's `user_data`,
release function, and queue. The rules above cover them, and anything more that
a C caller needs about them goes under `Lifetimes:`.

The generator spells C names and parameters as each binding's symbols, turns
`Returns:` and `Completes with:` into the binding's errors, and leaves
`Lifetimes:` in C.

```c
/**
 * Removes a source from the map's style.
 *
 * Completes with:
 * - MLN_STATUS_NOT_FOUND when the style has no source named `source_id`.
 * - MLN_STATUS_INVALID_STATE when a layer still uses the source.
 */
MLN_BINDING("execution=command")
MLN_API mln_status mln_map_remove_style_source(
  mln_map map, mln_buffer_view source_id, const mln_completion* completion,
  mln_diagnostic* out_diagnostic
) MLN_NOEXCEPT;

/**
 * Sets the headers of an HTTP request before the library sends it.
 *
 * `url` is the request's URL after any resource transform. The request waits
 * for the callback, so return quickly.
 *
 * Threads:
 * Runs on a library thread.
 *
 * Lifetimes:
 * `url` and `out_response` stay valid until the callback returns.
 */
MLN_BINDING("failure=MLN_STATUS_NATIVE_ERROR")
typedef mln_status (*mln_http_header_transform_callback)(
  void* user_data, mln_resource_kind kind, mln_buffer_view url,
  mln_http_header_transform_response* out_response MLN_BINDING("direction=out")
);
```
