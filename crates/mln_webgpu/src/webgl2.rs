//! Drawing on a device without storage buffers, which is what wgpu's WebGL2
//! backend offers. Built only with the `webgl` feature, and used only on
//! such a device ([`applies`]); the WGSL sources stay as they are.
//!
//! The shaders read a layer's per-drawable data from storage buffers
//! (`drawableVector`, `tilePropsVector`, `projectionVector`) at
//! `globalIndex.value`. [`rewrite`] declares each as a uniform array of
//! [`WINDOW`] elements instead. A drawable binds the window that holds its
//! element and passes its index within that window, so the buffers keep
//! their layout.

use std::fmt::Write as _;
use std::ops::Range;

use mln_util::logging::{Event, Log};

use crate::renderer_backend::BackendImpl;

/// The elements of a uniform array that stands in for a storage buffer.
/// Element sizes are multiples of 16 bytes, so every window of 16 starts at
/// a multiple of 256 bytes, which a uniform binding's offset has to be.
pub const WINDOW: u64 = 16;

/// The largest element: a window of them is WebGL2's largest uniform binding
/// (16 KiB).
pub const MAX_ELEMENT_SIZE: u64 = 1024;

/// What a uniform block's size has to be a multiple of on WebGL2.
const BLOCK_ALIGNMENT: u64 = 16;

/// Whether `device` has no storage buffers, so draws through this module.
pub(crate) fn applies(device: &wgpu::Device) -> bool {
    device.limits().max_storage_buffers_per_shader_stage == 0
}

/// The limits to request of a device: WebGL2's when the environment variable
/// `MLN_WEBGL2_LIMITS` is set, which draws through this module on any
/// backend, and `limits` otherwise.
pub(crate) fn limits_from_env(limits: wgpu::Limits, adapter: &wgpu::Adapter) -> wgpu::Limits {
    if std::env::var_os("MLN_WEBGL2_LIMITS").is_some() {
        wgpu::Limits::downlevel_webgl2_defaults().or_worse_values_from(&adapter.limits())
    } else {
        limits
    }
}

/// The size of the buffer for `size` bytes of uniform data, with room for
/// the last window to be whole. A buffer doesn't know its element size,
/// which is at most its data and [`MAX_ELEMENT_SIZE`].
pub(crate) fn buffer_size(size: usize) -> usize {
    size + (WINDOW as usize - 1) * size.min(MAX_ELEMENT_SIZE as usize)
}

/// The format and usage to create a texture with, from those every other
/// device gets and the usage the texture's owner set. WebGL2 renders to
/// float formats only with an extension, so a float texture is a render
/// attachment only if its owner said so, and a half-float render target has
/// 8 bits a channel where the device can't render to half floats.
pub(crate) fn texture(
    backend: &BackendImpl,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    owner_usage: wgpu::TextureUsages,
) -> (wgpu::TextureFormat, wgpu::TextureUsages) {
    use wgpu::TextureFormat as F;
    const TARGET: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if !backend.is_webgl2() || !matches!(format, F::R16Float | F::R32Float | F::Rgba16Float | F::Rgba32Float) {
        return (format, usage);
    }
    if !owner_usage.contains(TARGET) {
        return (format, usage - TARGET);
    }
    if format == F::Rgba16Float && !backend.half_float_renderable() {
        return (F::Rgba8Unorm, usage);
    }
    (format, usage)
}

/// A uniform array that stands in for a storage buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub group: u32,
    pub binding: u32,
    /// The size of an element in bytes.
    pub element_size: u64,
}

impl Window {
    /// The offset and size in bytes of the window that holds element
    /// `index`.
    pub fn range(&self, index: u32) -> (u64, u64) {
        let size = WINDOW * self.element_size;
        (u64::from(index) / WINDOW * size, size)
    }
}

/// A shader stage's source for a device without storage buffers.
#[derive(Debug)]
pub struct Rewritten {
    pub source: String,
    pub windows: Vec<Window>,
}

/// Declares the read-only storage arrays of `source` as uniform arrays of
/// [`WINDOW`] elements, and pads the structs of its other uniforms to a
/// multiple of 16 bytes. A storage array whose element this can't size, or
/// whose element doesn't fit a window, is logged and left as it is.
pub fn rewrite(source: &str) -> Rewritten {
    let mut out = String::with_capacity(source.len() + 256);
    let mut windows = Vec::new();
    for line in source.split_inclusive('\n') {
        let Some(array) = StorageArray::parse(line) else {
            out.push_str(line);
            continue;
        };
        match struct_size(source, array.element) {
            Some(size) if size.is_multiple_of(BLOCK_ALIGNMENT) && size <= MAX_ELEMENT_SIZE => {
                windows.push(Window { group: array.group, binding: array.binding, element_size: size });
                let _ = write!(out, "{}var<uniform>{}: array<{}, {WINDOW}>;", array.before, array.name, array.element);
                if line.ends_with('\n') {
                    out.push('\n');
                }
            }
            size => {
                Log::error(
                    Event::Shader,
                    format!("WebGL2: can't bind array<{}> in windows (element size {size:?})", array.element),
                );
                out.push_str(line);
            }
        }
    }
    pad_uniform_structs(&mut out);
    Rewritten { source: out, windows }
}

/// The size of `struct name` in `source` in bytes, or `None` when the struct
/// is missing or has a member this doesn't know the layout of.
pub fn struct_size(source: &str, name: &str) -> Option<u64> {
    let (end, align) = struct_extent(source, name, 0)?;
    Some(end.next_multiple_of(align))
}

/// The types of the uniform (not array) variables of `source`.
pub fn uniform_types(source: &str) -> Vec<&str> {
    const UNIFORM: &str = "var<uniform>";
    let mut types: Vec<&str> = Vec::new();
    for line in source.lines() {
        let Some(at) = line.find(UNIFORM) else { continue };
        let Some((_, ty)) = line[at + UNIFORM.len()..].split_once(':') else { continue };
        let ty = ty.trim().trim_end_matches(';').trim();
        if !ty.is_empty() && ty.chars().all(is_identifier) && !types.contains(&ty) {
            types.push(ty);
        }
    }
    types
}

/// `@group(G) @binding(B) var<storage, read> name: array<Element>;`
struct StorageArray<'a> {
    before: &'a str,
    name: &'a str,
    element: &'a str,
    group: u32,
    binding: u32,
}

impl<'a> StorageArray<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        const STORAGE: &str = "var<storage, read>";
        let at = line.find(STORAGE)?;
        let before = &line[..at];
        let (name, ty) = line[at + STORAGE.len()..].split_once(':')?;
        let element = ty.trim().strip_suffix(';')?.trim().strip_prefix("array<")?.strip_suffix('>')?.trim();
        if !element.chars().all(is_identifier) {
            return None;
        }
        Some(StorageArray {
            before,
            name,
            element,
            group: attribute(before, "@group")?,
            binding: attribute(before, "@binding")?,
        })
    }
}

/// The number in `@name(N)`.
fn attribute(text: &str, name: &str) -> Option<u32> {
    let rest = text[text.find(name)? + name.len()..].trim_start().strip_prefix('(')?;
    rest[..rest.find(')')?].trim().parse().ok()
}

fn is_identifier(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn pad_uniform_structs(source: &mut String) {
    let names: Vec<String> = uniform_types(source).into_iter().map(str::to_owned).collect();
    for name in names {
        let Some((end, align)) = struct_extent(source, &name, 0) else { continue };
        if end.next_multiple_of(align).is_multiple_of(BLOCK_ALIGNMENT) {
            continue;
        }
        let Some(body) = struct_body(source, &name) else { continue };
        let Some(last) = fields(&source[body.clone()]).pop() else { continue };
        let mut padding = String::new();
        if !last.comma {
            padding.push(',');
        }
        // Every member is a multiple of 4 bytes.
        for i in 0..(end.next_multiple_of(BLOCK_ALIGNMENT) - end) / 4 {
            let _ = write!(padding, "\n    webgl2_pad{i}: f32,");
        }
        source.insert_str(body.start + last.end, &padding);
    }
}

/// The body of `struct name { ... }` in `source`, between its braces.
fn struct_body(source: &str, name: &str) -> Option<Range<usize>> {
    const STRUCT: &str = "struct";
    let mut from = 0;
    while let Some(at) = source[from..].find(STRUCT) {
        let at = from + at;
        from = at + STRUCT.len();
        if source[..at].ends_with(is_identifier) {
            continue;
        }
        let Some(rest) = source[from..].trim_start().strip_prefix(name) else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('{') else { continue };
        let start = source.len() - rest.len();
        return Some(start..start + rest.find('}')?);
    }
    None
}

/// A member of a struct body.
struct Field {
    ty: String,
    /// Where the member ends in the body, after its comma if it has one.
    end: usize,
    comma: bool,
}

fn fields(body: &str) -> Vec<Field> {
    // Comments blanked, so that positions stay those of the body.
    let mut text = body.as_bytes().to_vec();
    let mut i = 0;
    while i + 1 < text.len() {
        if text[i] == b'/' && text[i + 1] == b'/' {
            while i < text.len() && text[i] != b'\n' {
                text[i] = b' ';
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    let Ok(text) = String::from_utf8(text) else {
        return Vec::new();
    };

    let mut fields = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in text.char_indices().chain([(text.len(), '\0')]) {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            ',' | '\0' if depth == 0 => {
                let member = &text[start..i];
                if let Some((name, ty)) = member.split_once(':') {
                    // An explicit layout isn't computed here.
                    let explicit = name.contains("@size") || name.contains("@align");
                    fields.push(Field {
                        ty: if explicit { String::new() } else { ty.trim().to_owned() },
                        end: if c == ',' { i + 1 } else { start + member.trim_end().len() },
                        comma: c == ',',
                    });
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    fields
}

/// Where the last member of `struct name` ends and the struct's alignment,
/// in bytes.
fn struct_extent(source: &str, name: &str, depth: u32) -> Option<(u64, u64)> {
    let body = struct_body(source, name)?;
    let fields = fields(&source[body]);
    if fields.is_empty() {
        return None;
    }
    let (mut end, mut align) = (0u64, 1u64);
    for field in fields {
        let (size, field_align) = layout(source, &field.ty, depth)?;
        end = end.next_multiple_of(field_align) + size;
        align = align.max(field_align);
    }
    Some((end, align))
}

/// The size and alignment of a WGSL type in bytes.
fn layout(source: &str, ty: &str, depth: u32) -> Option<(u64, u64)> {
    fn generic<'a>(ty: &'a str, name: &str) -> Option<&'a str> {
        Some(ty.strip_prefix(name)?.trim_start().strip_prefix('<')?.strip_suffix('>')?.trim())
    }
    fn vector(rows: &str, component: &str) -> Option<(u64, u64)> {
        if !matches!(component, "f32" | "u32" | "i32") {
            return None;
        }
        match rows {
            "2" => Some((8, 8)),
            "3" => Some((12, 16)),
            "4" => Some((16, 16)),
            _ => None,
        }
    }

    // A struct nested deeper than any shader's is a cycle.
    if depth > 8 {
        return None;
    }
    let ty = ty.trim();
    if matches!(ty, "f32" | "u32" | "i32") {
        return Some((4, 4));
    }
    if let Some(inner) = generic(ty, "array") {
        let (element, count) = inner.rsplit_once(',')?;
        let (size, align) = layout(source, element, depth + 1)?;
        return Some((count.trim().parse::<u64>().ok()? * size.next_multiple_of(align), align));
    }
    if let Some(shape) = ty.strip_prefix("vec")
        && let Some((rows, component)) = shape.split_once('<')
    {
        return vector(rows.trim(), component.strip_suffix('>')?.trim());
    }
    if let Some(shape) = ty.strip_prefix("mat")
        && let Some((shape, component)) = shape.split_once('<')
        && let Some((columns, rows)) = shape.trim().split_once('x')
    {
        let (size, align) = vector(rows, component.strip_suffix('>')?.trim())?;
        return Some((columns.parse::<u64>().ok()? * size.next_multiple_of(align), align));
    }
    if !ty.is_empty() && ty.chars().all(is_identifier) {
        let (end, align) = struct_extent(source, ty, depth + 1)?;
        return Some((end.next_multiple_of(align), align));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "\
struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};
struct Inner {
    matrix: mat4x4<f32>,
    stops: array<vec4<f32>, 4>,
    scale: vec2<f32>, // not the last
    ratio: f32,
};
struct Entry {
    data: Inner,
    pad: vec4<f32>,
};
struct Props {
    a: vec2<f32>,
    b: vec2<f32>,
    c: vec2<f32>,
    // more in the buffer
};
struct Bare { a: vec2<f32> }
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<Entry>;
@group(0) @binding(5) var<uniform> props: Props;
@group(0) @binding(6) var<uniform> bare: Bare;
";

    #[test]
    fn sizes_structs() {
        assert_eq!(struct_size(SOURCE, "GlobalIndexUBO"), Some(32));
        // 64 + 64 + 8 + 4 = 140, to its alignment of 16.
        assert_eq!(struct_size(SOURCE, "Inner"), Some(144));
        assert_eq!(struct_size(SOURCE, "Entry"), Some(160));
        assert_eq!(struct_size(SOURCE, "Props"), Some(24));
        assert_eq!(struct_size(SOURCE, "Bare"), Some(8));
        assert_eq!(struct_size(SOURCE, "Missing"), None);
        assert_eq!(struct_size("struct A { a: mat3x3<f32>, b: mat2x2<f32> }", "A"), Some(64));
        assert_eq!(struct_size("struct A { @size(32) a: f32 }", "A"), None);
    }

    #[test]
    fn declares_storage_arrays_as_windows() {
        let rewritten = rewrite(SOURCE);
        assert_eq!(rewritten.windows, [Window { group: 0, binding: 2, element_size: 160 }]);
        assert!(
            rewritten.source.contains("@group(0) @binding(2) var<uniform> drawableVector: array<Entry, 16>;\n"),
            "{}",
            rewritten.source
        );
        assert!(!rewritten.source.contains("storage"));
    }

    #[test]
    fn windows_start_at_multiples_of_their_size() {
        let window = Window { group: 0, binding: 2, element_size: 176 };
        assert_eq!(window.range(0), (0, 2816));
        assert_eq!(window.range(15), (0, 2816));
        assert_eq!(window.range(16), (2816, 2816));
        assert_eq!(window.range(40), (5632, 2816));
        // A buffer of 40 elements holds its third window whole.
        assert!(buffer_size(40 * 176) as u64 >= 5632 + 2816);
        assert!(buffer_size(176) as u64 >= 2816);
    }

    #[test]
    fn pads_uniform_structs_to_16_bytes() {
        let rewritten = rewrite(SOURCE).source;
        assert_eq!(struct_size(&rewritten, "Props"), Some(32));
        assert_eq!(struct_size(&rewritten, "Bare"), Some(16));
        assert!(
            rewritten.contains(
                "    c: vec2<f32>,\n    webgl2_pad0: f32,\n    webgl2_pad1: f32,\n    // more in the buffer\n};"
            ),
            "{rewritten}"
        );
        assert!(
            rewritten.contains("struct Bare { a: vec2<f32>,\n    webgl2_pad0: f32,\n    webgl2_pad1: f32, }"),
            "{rewritten}"
        );
        // Already a multiple of 16 bytes.
        assert!(rewritten.contains("    pad0: vec3<u32>,\n};"), "{rewritten}");
    }
}
