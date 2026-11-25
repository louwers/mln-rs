//! The text that a program's shaders compile from, in each shading
//! language: what a [`Program`](crate::shaders::program::Program) carries
//! in its [`sources`](crate::shaders::program::Program::sources), and each
//! backend reads in its language.
//!
//! A build keeps a language's text only with the feature of the language
//! (`wgsl`, `msl`, `glsl`, `glsl-es`), which the backend that reads the
//! language turns on: without it, the [`Sources`] setter of the language
//! drops the text, so that no program refers to it and the build leaves it
//! out.

use crate::shaders::block::Block;

/// A program's text in each language it has.
///
/// It is built from [`NONE`](Sources::NONE) with a setter for each
/// language, which keeps the text only where the language's feature is on.
/// A backend fails to compile a program without text in its language.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Sources {
    pub wgsl: Option<&'static WgslSource>,
    pub msl: Option<&'static MslSource>,
    pub glsl: Option<&'static GlslSource>,
    pub glsl_es: Option<&'static GlslEsSource>,
}

impl Sources {
    /// No text in any language.
    pub const NONE: Sources = Sources { wgsl: None, msl: None, glsl: None, glsl_es: None };

    /// It, with `source` as its WGSL where the `wgsl` feature is on.
    #[must_use]
    pub const fn wgsl(self, source: &'static WgslSource) -> Sources {
        if cfg!(feature = "wgsl") { Sources { wgsl: Some(source), ..self } } else { self }
    }

    /// It, with `source` as its MSL where the `msl` feature is on.
    #[must_use]
    pub const fn msl(self, source: &'static MslSource) -> Sources {
        if cfg!(feature = "msl") { Sources { msl: Some(source), ..self } } else { self }
    }

    /// It, with `source` as its GLSL where the `glsl` feature is on.
    #[must_use]
    pub const fn glsl(self, source: &'static GlslSource) -> Sources {
        if cfg!(feature = "glsl") { Sources { glsl: Some(source), ..self } } else { self }
    }

    /// It, with `source` as its GLSL ES where the `glsl-es` feature is on.
    #[must_use]
    pub const fn glsl_es(self, source: &'static GlslEsSource) -> Sources {
        if cfg!(feature = "glsl-es") { Sources { glsl_es: Some(source), ..self } } else { self }
    }
}

/// A program's WGSL, which `mln_webgpu` compiles.
///
/// Each stage follows the engine's prelude, the declarations of what the
/// program reads, which the backend writes from the program, with the
/// vertex stage's input in the vertex stage, and the program's own
/// prelude, and is preprocessed with the variant's defines (`#ifdef`,
/// `#ifndef`, `#else` and `#endif`).
#[derive(Debug)]
pub struct WgslSource {
    pub vertex: Option<&'static str>,
    pub fragment: Option<&'static str>,
    /// What both stages read after the declarations.
    pub prelude: Option<&'static str>,
}

/// A program's MSL, which `mln_mtl` compiles: its main functions and what
/// they call.
///
/// Each main function's parameters end with a placeholder,
/// `MLN_VERTEX_BINDINGS` or `MLN_FRAGMENT_BINDINGS`, where the backend puts
/// the parameters of what its stage reads. The variant's defines are the
/// library's preprocessor macros.
#[derive(Debug)]
pub struct MslSource {
    pub vertex_main_function: &'static str,
    pub fragment_main_function: &'static str,
    pub source: &'static str,
    /// Whether its library compiles with fast math, Metal's default, which
    /// lets the compiler fuse and reorder floating-point operations.
    pub fast_math: bool,
}

/// A program's GLSL, for naga's GLSL front end, which `mln_glsl` composes
/// for Vulkan and D3D12: its stages, which read the program's buffers,
/// textures and attributes by the names of the declarations that the
/// backend writes from the program.
#[derive(Debug)]
pub struct GlslSource {
    pub vertex: &'static str,
    pub fragment: &'static str,
}

/// A program's GLSL ES 3.00, which `mln_gl` composes for OpenGL ES, and as
/// GLSL 3.30 for desktop OpenGL: its stages, which read what the program's
/// GL ES declarations name, and how those declarations qualify and name
/// what it reads.
#[derive(Debug)]
pub struct GlslEsSource {
    pub vertex: &'static str,
    pub fragment: &'static str,
    pub declarations: &'static GlslEsDeclarations,
}

/// The precisions that GL ES reads a program's values at, and the names of
/// the members of its blocks that GL ES names otherwise. They are
/// MapLibre's, which keep paint values that don't need more in `lowp` or
/// `mediump` registers on phones' GPUs. Only GL ES has precision
/// qualifiers. What they don't name is `highp`, but a pad, which no shader
/// reads, is `lowp`, and a sampler has GL ES's default, `lowp`.
#[derive(Debug)]
pub struct GlslEsDeclarations {
    /// The members of the program's blocks that GL ES reads at less than
    /// `highp`, but pads, by block.
    pub members: &'static [(&'static Block, Precision, &'static [&'static str])],
    /// The attributes that GL ES reads at less than `highp`.
    pub attributes: &'static [(Precision, &'static [&'static str])],
    /// The textures that GL ES samples at `highp`, such as DEMs, whose
    /// texels encode heights in their bytes.
    pub highp_samplers: &'static [&'static str],
    /// The GL ES names of the members of the program's blocks whose
    /// `u_<member>` another block of a program that has theirs has too: the
    /// block, the member and its name.
    pub names: &'static [(&'static Block, &'static str, &'static str)],
}

impl GlslEsDeclarations {
    /// Every value at `highp`, every sampler at GL ES's default, and every
    /// member as `u_<member>`.
    pub const DEFAULT: GlslEsDeclarations =
        GlslEsDeclarations { members: &[], attributes: &[], highp_samplers: &[], names: &[] };
}

/// A GL ES precision qualifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    Low,
    Medium,
    High,
}

impl Precision {
    /// Its qualifier.
    #[must_use]
    pub const fn qualifier(self) -> &'static str {
        match self {
            Precision::Low => "lowp",
            Precision::Medium => "mediump",
            Precision::High => "highp",
        }
    }
}
