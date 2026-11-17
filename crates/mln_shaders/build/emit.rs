//! Lowers a program linked with its variant flags kept into the text the
//! crate embeds, and reads the tables of its resources and vertex inputs.
//!
//! Each kept `@if` becomes `#if` and `#endif` lines around its declaration,
//! struct member or statement, and each global declaration takes the
//! condition under which an entry point reaches it, so that a variant's
//! lines are what WESL links for the variant. A resource's `@group` and
//! `@binding` become the macros `__mln_group_<n>` and `__mln_binding_<n>`
//! of its index `n` in the table, which the crate defines for the target's
//! binding convention.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use wesl::syntax::{
    AddressSpace, Attribute, AttributeNode, CustomAttribute, Declaration, DeclarationKind, Expression, FormalParameter,
    FunctionCall, FunctionCallStatement, GlobalDeclaration, GlobalDeclarationNode, Ident, LiteralExpression, Statement,
    StatementNode, Struct, StructMember, TemplateArg, TranslationUnit, TypeExpression, Visibility,
};

use crate::analyze::{self, cond_of};
use crate::cond::Cond;
use crate::normalize::statement_attributes;

/// A resource, as the WESL declares it.
pub(crate) struct Resource {
    pub(crate) name: String,
    /// `Uniform`, `Storage`, `Texture` or `Sampler`.
    pub(crate) kind: &'static str,
    pub(crate) group: u32,
    pub(crate) binding: u32,
    /// The bytes of a buffer, of an element of a storage buffer.
    pub(crate) size: u64,
    /// When each stage uses it.
    pub(crate) vertex: Cond,
    pub(crate) fragment: Cond,
    /// When a texture is filterable: sampled, or never loaded.
    pub(crate) filterable: Cond,
}

/// A vertex input.
pub(crate) struct Input {
    pub(crate) location: u32,
    /// `Float`, `Sint` or `Uint`.
    pub(crate) scalar: &'static str,
    pub(crate) components: u32,
    pub(crate) cond: Cond,
}

pub(crate) struct Emitted {
    pub(crate) text: String,
    pub(crate) resources: Vec<Resource>,
    pub(crate) inputs: Vec<Input>,
}

/// Lowers `unit`, linked for OpenGL if `gl`.
pub(crate) fn emit(mut unit: TranslationUnit, gl: bool) -> Result<Emitted, String> {
    unit.global_declarations.retain(|node| !matches!(node.node(), GlobalDeclaration::Void));
    let decls = analyze::decls(&unit)?;
    let vertex = analyze::reach(&decls, &["vs_main"]);
    let fragment = analyze::reach(&decls, &["fs_main"]);
    let any: Vec<Cond> = vertex.iter().zip(&fragment).map(|(v, f)| v.clone().or(f.clone())).collect();
    let texture_use = |texture: &str, sampled: bool| {
        Cond::any(decls.iter().zip(&any).flat_map(|(decl, reach)| {
            decl.textures
                .iter()
                .filter(move |(name, s, _)| name == texture && *s == sampled)
                .map(move |(_, _, cond)| reach.clone().and(cond.clone()))
        }))
    };

    let mut structs: BTreeMap<String, Vec<&Struct>> = BTreeMap::new();
    for node in &unit.global_declarations {
        if let GlobalDeclaration::Struct(decl) = node.node() {
            structs.entry(decl.ident.name().clone()).or_default().push(decl);
        }
    }

    let mut resources = Vec::new();
    let mut resource_of = vec![None; unit.global_declarations.len()];
    for (index, node) in unit.global_declarations.iter().enumerate() {
        let GlobalDeclaration::Declaration(decl) = node.node() else { continue };
        let DeclarationKind::Var(space) = decl.kind else { continue };
        let name = decl.ident.name().clone();
        let (Some(group), Some(binding)) = (integer(&decl.attributes, true)?, integer(&decl.attributes, false)?) else {
            continue;
        };
        if any[index].is_false() {
            continue;
        }
        let ty = decl.ty.as_ref().ok_or_else(|| format!("{name} has no type"))?;
        let type_name = ty.ident.name().clone();
        let (kind, size) = match space {
            Some((AddressSpace::Uniform, _)) => ("Uniform", layout(ty, &structs)?.1),
            Some((AddressSpace::Storage, _)) => {
                let element = array_element(ty).ok_or_else(|| format!("{name}: a storage buffer is an array"))?;
                ("Storage", stride(layout(element, &structs)?))
            }
            None if type_name.starts_with("texture") => ("Texture", 0),
            None if type_name.starts_with("sampler") => ("Sampler", 0),
            _ => return Err(format!("{name} is no buffer, texture or sampler")),
        };
        let filterable = if kind == "Texture" {
            texture_use(&name, true).or(texture_use(&name, false).not())
        } else {
            crate::cond::TRUE
        };
        resource_of[index] = Some(resources.len());
        resources.push(Resource {
            name,
            kind,
            group,
            binding,
            size,
            vertex: vertex[index].clone(),
            fragment: fragment[index].clone(),
            filterable,
        });
    }

    let inputs = inputs(&unit, &structs)?;

    let mut conds = Vec::new();
    let mut declarations = Vec::new();
    for (index, mut node) in std::mem::take(&mut unit.global_declarations).into_iter().enumerate() {
        if any[index].is_false() {
            continue;
        }
        if let Some(resource) = resource_of[index] {
            rewrite_resource(&mut node, resource, gl && resources[resource].kind == "Storage")?;
        }
        match node.node_mut() {
            GlobalDeclaration::Struct(decl) => {
                let mut members = Vec::new();
                for mut member in std::mem::take(&mut decl.members) {
                    let cond = cond_of(&member.attributes)?;
                    let end = mark(&mut member.node_mut().attributes, &cond, &mut conds);
                    members.push(member);
                    if let Some(end) = end {
                        members.push(StructMember { attributes: Vec::new(), ident: Ident::new(end), ty: f32() }.into());
                    }
                }
                decl.members = members;
            }
            GlobalDeclaration::Function(decl) => {
                let mut parameters = Vec::new();
                for mut parameter in std::mem::take(&mut decl.parameters) {
                    let cond = cond_of(&parameter.attributes)?;
                    let end = mark(&mut parameter.attributes, &cond, &mut conds);
                    parameters.push(parameter);
                    if let Some(end) = end {
                        parameters.push(FormalParameter { attributes: Vec::new(), ident: Ident::new(end), ty: f32() });
                    }
                }
                decl.parameters = parameters;
                statements(&mut decl.body.statements, &mut conds)?;
            }
            _ => {}
        }
        let attributes = declaration_attributes(&mut node);
        attributes.retain(|attribute| !attribute.is_condcomp());
        let end = mark(attributes, &any[index], &mut conds);
        declarations.push(node);
        if let Some(end) = end {
            let marker = Declaration {
                attributes: Vec::new(),
                visibility: Visibility::Package,
                kind: DeclarationKind::Const,
                ident: Ident::new(end),
                ty: None,
                initializer: Some(Expression::Literal(LiteralExpression::AbstractInt(0)).into()),
            };
            declarations.push(GlobalDeclaration::Declaration(marker).into());
        }
    }
    unit.global_declarations = declarations;
    let text = lower(&unit.to_string(), &conds)?;
    Ok(Emitted { text, resources, inputs })
}

/// Marks a node of condition `cond` with `@mln_if_<n>`, `n` its index in
/// `conds`, and returns the name its end marker takes, unless `cond` is
/// true.
fn mark(attributes: &mut Vec<AttributeNode>, cond: &Cond, conds: &mut Vec<String>) -> Option<String> {
    attributes.retain(|attribute| !attribute.is_condcomp());
    if cond.is_true() {
        return None;
    }
    let index = conds.len();
    conds.push(cond.to_string());
    let attribute = Attribute::Custom(CustomAttribute { name: format!("mln_if_{index}"), arguments: None });
    attributes.insert(0, attribute.into());
    Some(format!("mln_endif_{index}"))
}

/// Marks the conditional statements of `statements` and of those inside
/// them.
fn statements(statements: &mut Vec<StatementNode>, conds: &mut Vec<String>) -> Result<(), String> {
    let mut marked = Vec::with_capacity(statements.len());
    for mut node in std::mem::take(statements) {
        let Some(attributes) = statement_attributes(&mut node) else { continue };
        let cond = cond_of(attributes)?;
        let end = mark(attributes, &cond, conds);
        match node.node_mut() {
            Statement::Compound(stmt) => self::statements(&mut stmt.statements, conds)?,
            Statement::If(stmt) => {
                self::statements(&mut stmt.if_clause.body.statements, conds)?;
                for clause in &mut stmt.else_if_clauses {
                    self::statements(&mut clause.body.statements, conds)?;
                }
                if let Some(clause) = &mut stmt.else_clause {
                    self::statements(&mut clause.body.statements, conds)?;
                }
            }
            Statement::Switch(stmt) => {
                for clause in &mut stmt.clauses {
                    self::statements(&mut clause.body.statements, conds)?;
                }
            }
            Statement::Loop(stmt) => {
                self::statements(&mut stmt.body.statements, conds)?;
                if let Some(continuing) = &mut stmt.continuing {
                    self::statements(&mut continuing.body.statements, conds)?;
                }
            }
            Statement::For(stmt) => self::statements(&mut stmt.body.statements, conds)?,
            Statement::While(stmt) => self::statements(&mut stmt.body.statements, conds)?,
            _ => {}
        }
        marked.push(node);
        if let Some(end) = end {
            let call = FunctionCall {
                ty: TypeExpression { path: None, ident: Ident::new(end), template_args: None },
                arguments: Vec::new(),
            };
            marked.push(Statement::FunctionCall(FunctionCallStatement { attributes: Vec::new(), call }).into());
        }
    }
    *statements = marked;
    Ok(())
}

/// The type of an end marker.
fn f32() -> TypeExpression {
    TypeExpression { path: None, ident: Ident::new("f32".to_owned()), template_args: None }
}

/// `printed`, a unit printed with its markers, with each marker a line of
/// the preprocessor: `@mln_if_<n>` before a node, `mln_endif_<n>` as the
/// declaration, member, parameter or statement after it.
fn lower(printed: &str, conds: &[String]) -> Result<String, String> {
    const IF: &str = "@mln_if_";
    const END: &str = "mln_endif_";
    let mut text = String::with_capacity(printed.len());
    for line in printed.lines() {
        let mut rest = line.trim_start();
        let indent = &line[..line.len() - rest.len()];
        if rest.is_empty() {
            text.push('\n');
            continue;
        }
        let flush = |pending: &str, text: &mut String| {
            if !pending.trim().is_empty() {
                text.push_str(indent);
                text.push_str(pending.trim());
                text.push('\n');
            }
        };
        loop {
            let next = [rest.find(IF), rest.find(END)].into_iter().flatten().min();
            let Some(at) = next else {
                flush(rest, &mut text);
                break;
            };
            let is_if = rest[at..].starts_with(IF);
            let prefix = &rest[..at];
            flush(if is_if { prefix } else { prefix.strip_suffix("const ").unwrap_or(prefix) }, &mut text);
            let after = &rest[at + if is_if { IF.len() } else { END.len() }..];
            let digits = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
            let index: usize = after[..digits].parse().map_err(|_| format!("a marker without its index: {line}"))?;
            rest = &after[digits..];
            if is_if {
                let _ = writeln!(text, "{indent}#if {}", conds[index]);
            } else {
                let _ = writeln!(text, "{indent}#endif");
                // The rest of the end marker, and the separator after it.
                while let Some(tail) = [": f32", " = 0", "()", ";", ",", " "].iter().find_map(|t| rest.strip_prefix(t))
                {
                    rest = tail;
                }
            }
            rest = rest.trim_start();
        }
    }
    if let Some(line) =
        text.lines().find(|line| ["mln_if_", END, "@if", "@elif", "@else"].iter().any(|m| line.contains(m)))
    {
        return Err(format!("a condition the lowering missed: {line}"));
    }
    Ok(text)
}

fn declaration_attributes(node: &mut GlobalDeclarationNode) -> &mut Vec<AttributeNode> {
    match node.node_mut() {
        GlobalDeclaration::Declaration(decl) => &mut decl.attributes,
        GlobalDeclaration::TypeAlias(decl) => &mut decl.attributes,
        GlobalDeclaration::Struct(decl) => &mut decl.attributes,
        GlobalDeclaration::Function(decl) => &mut decl.attributes,
        GlobalDeclaration::ConstAssert(decl) => &mut decl.attributes,
        GlobalDeclaration::Compound(decl) => &mut decl.attributes,
        GlobalDeclaration::Void => unreachable!("removed"),
    }
}

/// The value of `@group` if `group`, else of `@binding`, if any: an integer
/// literal.
fn integer(attributes: &[AttributeNode], group: bool) -> Result<Option<u32>, String> {
    let expression = attributes.iter().find_map(|attribute| match attribute.node() {
        Attribute::Group(expression) if group => Some(expression),
        Attribute::Binding(expression) if !group => Some(expression),
        _ => None,
    });
    expression
        .map(|expression| {
            literal(expression).ok_or_else(|| format!("@group and @binding take integer literals, not {expression}"))
        })
        .transpose()
}

fn literal(expression: &Expression) -> Option<u32> {
    match expression {
        Expression::Literal(LiteralExpression::AbstractInt(value)) => u32::try_from(*value).ok(),
        Expression::Literal(LiteralExpression::I32(value)) => u32::try_from(*value).ok(),
        Expression::Literal(LiteralExpression::U32(value)) => Some(*value),
        _ => None,
    }
}

/// Puts the macros of resource `index` in `node`'s `@group` and `@binding`,
/// and on OpenGL, which has no storage buffers, makes a `storage` buffer a
/// uniform block of one element: the draw's.
fn rewrite_resource(node: &mut GlobalDeclarationNode, index: usize, uniform_block: bool) -> Result<(), String> {
    let GlobalDeclaration::Declaration(decl) = node.node_mut() else { unreachable!("a resource is a declaration") };
    let ident = |name: String| {
        Expression::TypeOrIdentifier(TypeExpression { path: None, ident: Ident::new(name), template_args: None })
    };
    for attribute in &mut decl.attributes {
        match attribute.node_mut() {
            Attribute::Group(expression) => *expression = ident(format!("__mln_group_{index}")).into(),
            Attribute::Binding(expression) => *expression = ident(format!("__mln_binding_{index}")).into(),
            _ => {}
        }
    }
    if uniform_block {
        decl.kind = DeclarationKind::Var(Some((AddressSpace::Uniform, None)));
        let args = decl
            .ty
            .as_mut()
            .and_then(|ty| ty.template_args.as_mut())
            .ok_or_else(|| format!("{}: a storage buffer is an array", decl.ident))?;
        args.push(TemplateArg { expression: Expression::Literal(LiteralExpression::AbstractInt(1)).into() });
    }
    Ok(())
}

/// The element type of `ty`, an array of any length.
fn array_element(ty: &TypeExpression) -> Option<&TypeExpression> {
    if *ty.ident.name() != "array" {
        return None;
    }
    match ty.template_args.as_deref() {
        Some([element]) => type_of(&element.expression),
        _ => None,
    }
}

fn type_of(expression: &Expression) -> Option<&TypeExpression> {
    match expression {
        Expression::TypeOrIdentifier(ty) => Some(ty),
        _ => None,
    }
}

/// The stride of an array of elements of `(align, size)`.
fn stride((align, size): (u64, u64)) -> u64 {
    size.next_multiple_of(align)
}

/// The scalar of a type name and its count, for the scalars, vectors and
/// matrices: `(scalar, columns, rows)`.
fn shape(ty: &TypeExpression) -> Option<(&'static str, u64, u64)> {
    let name = ty.ident.name().clone();
    let scalar = |name: &str| match name {
        "f32" | "f" => Some("f32"),
        "i32" | "i" => Some("i32"),
        "u32" | "u" => Some("u32"),
        "f16" | "h" => Some("f16"),
        _ => None,
    };
    if let Some(scalar) = scalar(&name) {
        return Some((scalar, 1, 1));
    }
    let templated = || {
        let args = ty.template_args.as_deref()?;
        scalar(&type_of(&args.first()?.expression)?.ident.name())
    };
    let digit = |c: u8| u64::from(c - b'0');
    let bytes = name.as_bytes();
    if let Some(rest) = name.strip_prefix("vec") {
        let rows = digit(*rest.as_bytes().first()?);
        let scalar = if rest.len() == 1 { templated()? } else { scalar(&rest[1..])? };
        return Some((scalar, 1, rows));
    }
    if name.starts_with("mat") && bytes.len() >= 6 && bytes[4] == b'x' {
        let (columns, rows) = (digit(bytes[3]), digit(bytes[5]));
        let scalar = if bytes.len() == 6 { templated()? } else { scalar(&name[6..])? };
        return Some((scalar, columns, rows));
    }
    None
}

/// The alignment and size of `ty` in a buffer, by WGSL's layout rules.
fn layout(ty: &TypeExpression, structs: &BTreeMap<String, Vec<&Struct>>) -> Result<(u64, u64), String> {
    let name = ty.ident.name().clone();
    if let Some((scalar, columns, rows)) = shape(ty) {
        let bytes = if scalar == "f16" { 2 } else { 4 };
        let vector = |rows: u64| match rows {
            1 => (bytes, bytes),
            2 => (2 * bytes, 2 * bytes),
            3 => (4 * bytes, 3 * bytes),
            _ => (4 * bytes, 4 * bytes),
        };
        let column = vector(rows);
        return Ok(if columns == 1 { column } else { (column.0, columns * stride(column)) });
    }
    if name == "array" {
        let args = ty.template_args.as_deref().unwrap_or_default();
        let element = args.first().and_then(|arg| type_of(&arg.expression)).ok_or("an array of no type")?;
        let element = layout(element, structs)?;
        let count = match args.get(1) {
            Some(arg) => u64::from(literal(&arg.expression).ok_or("an array of a length that isn't a literal")?),
            None => 1,
        };
        return Ok((element.0, count * stride(element)));
    }
    let decl = match structs.get(&name).map(Vec::as_slice) {
        Some([decl]) => decl,
        Some(_) => return Err(format!("{name}: a buffer of a struct declared more than once")),
        None => return Err(format!("{name}: a buffer of a type without a layout")),
    };
    let (mut align, mut end) = (1u64, 0u64);
    for member in &decl.members {
        if !cond_of(&member.attributes)?.is_true() {
            return Err(format!("{name}: a buffer of a struct with a conditional member"));
        }
        let (mut member_align, mut member_size) = layout(&member.ty, structs)?;
        for attribute in &member.attributes {
            match attribute.node() {
                Attribute::Align(e) => member_align = u64::from(literal(e).ok_or("@align of a literal")?),
                Attribute::Size(e) => member_size = u64::from(literal(e).ok_or("@size of a literal")?),
                _ => {}
            }
        }
        end = end.next_multiple_of(member_align) + member_size;
        align = align.max(member_align);
    }
    Ok((align, end.next_multiple_of(align)))
}

/// The vertex inputs of `unit`'s `vs_main`, in order of location.
fn inputs(unit: &TranslationUnit, structs: &BTreeMap<String, Vec<&Struct>>) -> Result<Vec<Input>, String> {
    let entry = unit
        .global_declarations
        .iter()
        .find_map(|node| match node.node() {
            GlobalDeclaration::Function(decl) if *decl.ident.name() == "vs_main" => Some(decl),
            _ => None,
        })
        .ok_or("no vs_main")?;
    let mut inputs = Vec::new();
    let mut add = |attributes: &[AttributeNode], ty: &TypeExpression, cond: Cond| -> Result<(), String> {
        let location = attributes.iter().find_map(|attribute| match attribute.node() {
            Attribute::Location(e) => Some(literal(e).ok_or("@location of a literal")),
            _ => None,
        });
        let Some(location) = location.transpose()? else { return Ok(()) };
        let (scalar, columns, rows) = shape(ty).ok_or_else(|| format!("an input of type {ty}"))?;
        if columns != 1 {
            return Err(format!("an input of type {ty}"));
        }
        let scalar = match scalar {
            "i32" => "Sint",
            "u32" => "Uint",
            _ => "Float",
        };
        inputs.push(Input { location, scalar, components: rows as u32, cond });
        Ok(())
    };
    for parameter in &entry.parameters {
        match structs.get(&*parameter.ty.ident.name()).map(Vec::as_slice) {
            Some([decl]) => {
                for member in &decl.members {
                    add(&member.attributes, &member.ty, cond_of(&member.attributes)?)?;
                }
            }
            Some(_) => return Err("an input struct declared more than once".to_owned()),
            None => add(&parameter.attributes, &parameter.ty, cond_of(&parameter.attributes)?)?,
        }
    }
    inputs.sort_by_key(|input| input.location);
    Ok(inputs)
}
