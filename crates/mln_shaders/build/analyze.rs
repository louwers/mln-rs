//! What a program linked with its variant flags kept references, and under
//! which flags each declaration is reachable from each entry point: what
//! WESL keeps of it when it links a variant, and what naga finds that each
//! stage uses.

use std::collections::BTreeMap;

use wesl::syntax::{
    Attribute, AttributeNode, Expression, GlobalDeclaration, Statement, StatementNode, TranslationUnit, TypeExpression,
};

use crate::cond::{Cond, FALSE, TRUE};

/// A global declaration of a linked program.
pub(crate) struct Decl {
    pub(crate) name: String,
    /// Its own `@if`.
    pub(crate) cond: Cond,
    /// The names it references, each under the condition of the statement
    /// or member that does.
    refs: Vec<(String, Cond)>,
    /// The textures it samples (`true`) or loads texels of (`false`), each
    /// under the condition of the statement that does.
    pub(crate) textures: Vec<(String, bool, Cond)>,
}

/// The condition of a node: its `@if`, or true.
pub(crate) fn cond_of(attributes: &[AttributeNode]) -> Result<Cond, String> {
    let mut cond = TRUE;
    for attribute in attributes {
        match attribute.node() {
            Attribute::If(expression) => cond = cond.and(Cond::of(expression)?),
            Attribute::Elif(_) | Attribute::Else => return Err("an @elif or @else after normalizing".to_owned()),
            _ => {}
        }
    }
    Ok(cond)
}

/// Each global declaration of `unit`, in order.
pub(crate) fn decls(unit: &TranslationUnit) -> Result<Vec<Decl>, String> {
    let mut decls = Vec::new();
    for node in &unit.global_declarations {
        let mut walker = Walker::default();
        let (name, attributes) = match node.node() {
            GlobalDeclaration::Void => continue,
            GlobalDeclaration::Declaration(decl) => {
                walker.attributes(&decl.attributes, &TRUE);
                if let Some(ty) = &decl.ty {
                    walker.ty(ty, &TRUE);
                }
                if let Some(init) = &decl.initializer {
                    walker.expression(init, &TRUE);
                }
                (decl.ident.name().clone(), &decl.attributes)
            }
            GlobalDeclaration::TypeAlias(decl) => {
                walker.ty(&decl.ty, &TRUE);
                (decl.ident.name().clone(), &decl.attributes)
            }
            GlobalDeclaration::Struct(decl) => {
                for member in &decl.members {
                    let cond = cond_of(&member.attributes)?;
                    walker.attributes(&member.attributes, &cond);
                    walker.ty(&member.ty, &cond);
                }
                (decl.ident.name().clone(), &decl.attributes)
            }
            GlobalDeclaration::Function(decl) => {
                walker.attributes(&decl.attributes, &TRUE);
                for parameter in &decl.parameters {
                    let cond = cond_of(&parameter.attributes)?;
                    walker.attributes(&parameter.attributes, &cond);
                    walker.ty(&parameter.ty, &cond);
                    walker.locals.push(parameter.ident.name().clone());
                }
                walker.attributes(&decl.return_attributes, &TRUE);
                if let Some(ty) = &decl.return_type {
                    walker.ty(ty, &TRUE);
                }
                walker.statements(&decl.body.statements, &TRUE)?;
                (decl.ident.name().clone(), &decl.attributes)
            }
            GlobalDeclaration::ConstAssert(_) => return Err("a const_assert".to_owned()),
            GlobalDeclaration::Compound(_) => return Err("a compound declaration after normalizing".to_owned()),
        };
        let cond = cond_of(attributes).map_err(|error| format!("{name}: {error}"))?;
        decls.push(Decl { name, cond, refs: walker.refs, textures: walker.textures });
    }
    Ok(decls)
}

/// The condition under which each of `decls` is reachable from the
/// functions named `roots`.
pub(crate) fn reach(decls: &[Decl], roots: &[&str]) -> Vec<Cond> {
    let mut referrers: BTreeMap<&str, Vec<(usize, &Cond)>> = BTreeMap::new();
    for (index, decl) in decls.iter().enumerate() {
        for (name, cond) in &decl.refs {
            referrers.entry(name).or_default().push((index, cond));
        }
    }
    let mut reach =
        Reach { decls, roots, referrers, memo: vec![None; decls.len()], visiting: vec![false; decls.len()] };
    (0..decls.len()).map(|index| reach.of(index)).collect()
}

struct Reach<'a> {
    decls: &'a [Decl],
    roots: &'a [&'a str],
    referrers: BTreeMap<&'a str, Vec<(usize, &'a Cond)>>,
    memo: Vec<Option<Cond>>,
    visiting: Vec<bool>,
}

impl Reach<'_> {
    fn of(&mut self, index: usize) -> Cond {
        if let Some(cond) = &self.memo[index] {
            return cond.clone();
        }
        // WGSL has no recursion; a cycle is a local named as a global.
        if self.visiting[index] {
            return FALSE;
        }
        self.visiting[index] = true;
        let decl = &self.decls[index];
        let from = if self.roots.contains(&decl.name.as_str()) {
            TRUE
        } else {
            let referrers = self.referrers.get(decl.name.as_str()).cloned().unwrap_or_default();
            Cond::any(referrers.into_iter().map(|(referrer, cond)| self.of(referrer).and(cond.clone())))
        };
        let cond = decl.cond.clone().and(from);
        self.visiting[index] = false;
        self.memo[index] = Some(cond.clone());
        cond
    }
}

/// Collects what a declaration references.
#[derive(Default)]
struct Walker {
    refs: Vec<(String, Cond)>,
    textures: Vec<(String, bool, Cond)>,
    /// The names of the parameters and local declarations in scope, which
    /// hide globals of the same names.
    locals: Vec<String>,
}

impl Walker {
    fn reference(&mut self, name: &str, cond: &Cond) {
        if self.locals.iter().any(|local| local == name) {
            return;
        }
        if !self.refs.iter().any(|(n, c)| n == name && c == cond) {
            self.refs.push((name.to_owned(), cond.clone()));
        }
    }

    fn attributes(&mut self, attributes: &[AttributeNode], cond: &Cond) {
        for attribute in attributes {
            match attribute.node() {
                Attribute::Align(e)
                | Attribute::Binding(e)
                | Attribute::Group(e)
                | Attribute::Id(e)
                | Attribute::Location(e)
                | Attribute::Size(e)
                | Attribute::BlendSrc(e) => self.expression(e, cond),
                _ => {}
            }
        }
    }

    fn ty(&mut self, ty: &TypeExpression, cond: &Cond) {
        self.reference(&ty.ident.name(), cond);
        for arg in ty.template_args.iter().flatten() {
            self.expression(&arg.expression, cond);
        }
    }

    fn expression(&mut self, expression: &Expression, cond: &Cond) {
        match expression {
            Expression::Literal(_) => {}
            Expression::Parenthesized(e) => self.expression(&e.expression, cond),
            Expression::NamedComponent(e) => self.expression(&e.base, cond),
            Expression::Indexing(e) => {
                self.expression(&e.base, cond);
                self.expression(&e.index, cond);
            }
            Expression::Unary(e) => self.expression(&e.operand, cond),
            Expression::Binary(e) => {
                self.expression(&e.left, cond);
                self.expression(&e.right, cond);
            }
            Expression::FunctionCall(call) => {
                let function = call.ty.ident.name().clone();
                let sampled = function.starts_with("textureSample") || function.starts_with("textureGather");
                if (sampled || function == "textureLoad")
                    && let Some(Expression::TypeOrIdentifier(texture)) =
                        call.arguments.first().map(wesl::syntax::Spanned::node)
                {
                    self.textures.push((texture.ident.name().clone(), sampled, cond.clone()));
                }
                self.ty(&call.ty, cond);
                for arg in &call.arguments {
                    self.expression(arg, cond);
                }
            }
            Expression::TypeOrIdentifier(ty) => self.ty(ty, cond),
        }
    }

    /// Walks `statements`, a scope.
    fn statements(&mut self, statements: &[StatementNode], cond: &Cond) -> Result<(), String> {
        let scope = self.locals.len();
        for statement in statements {
            self.statement(statement, cond)?;
        }
        self.locals.truncate(scope);
        Ok(())
    }

    fn statement(&mut self, statement: &StatementNode, outer: &Cond) -> Result<(), String> {
        let cond = &outer.clone().and(cond_of(statement_attributes(statement))?);
        match statement.node() {
            Statement::Void | Statement::Break(_) | Statement::Continue(_) | Statement::Discard(_) => {}
            Statement::Compound(stmt) => self.statements(&stmt.statements, cond)?,
            Statement::Assignment(stmt) => {
                self.expression(&stmt.lhs, cond);
                self.expression(&stmt.rhs, cond);
            }
            Statement::Increment(stmt) => self.expression(&stmt.expression, cond),
            Statement::Decrement(stmt) => self.expression(&stmt.expression, cond),
            Statement::If(stmt) => {
                self.expression(&stmt.if_clause.expression, cond);
                self.statements(&stmt.if_clause.body.statements, cond)?;
                for clause in &stmt.else_if_clauses {
                    self.expression(&clause.expression, cond);
                    self.statements(&clause.body.statements, cond)?;
                }
                if let Some(clause) = &stmt.else_clause {
                    self.statements(&clause.body.statements, cond)?;
                }
            }
            Statement::Switch(stmt) => {
                self.expression(&stmt.expression, cond);
                for clause in &stmt.clauses {
                    for selector in &clause.case_selectors {
                        if let wesl::syntax::CaseSelector::Expression(e) = selector {
                            self.expression(e, cond);
                        }
                    }
                    self.statements(&clause.body.statements, cond)?;
                }
            }
            Statement::Loop(stmt) => {
                self.statements(&stmt.body.statements, cond)?;
                if let Some(continuing) = &stmt.continuing {
                    self.statements(&continuing.body.statements, cond)?;
                    if let Some(break_if) = &continuing.break_if {
                        self.expression(&break_if.expression, cond);
                    }
                }
            }
            Statement::For(stmt) => {
                let scope = self.locals.len();
                if let Some(init) = &stmt.initializer {
                    self.statement(init, cond)?;
                }
                if let Some(condition) = &stmt.condition {
                    self.expression(condition, cond);
                }
                if let Some(update) = &stmt.update {
                    self.statement(update, cond)?;
                }
                self.statements(&stmt.body.statements, cond)?;
                self.locals.truncate(scope);
            }
            Statement::While(stmt) => {
                self.expression(&stmt.condition, cond);
                self.statements(&stmt.body.statements, cond)?;
            }
            Statement::Return(stmt) => {
                if let Some(e) = &stmt.expression {
                    self.expression(e, cond);
                }
            }
            Statement::FunctionCall(stmt) => self.expression(&Expression::FunctionCall(stmt.call.clone()), cond),
            Statement::ConstAssert(stmt) => self.expression(&stmt.expression, cond),
            Statement::Declaration(stmt) => {
                if let Some(ty) = &stmt.ty {
                    self.ty(ty, cond);
                }
                if let Some(init) = &stmt.initializer {
                    self.expression(init, cond);
                }
                self.locals.push(stmt.ident.name().clone());
            }
        }
        Ok(())
    }
}

/// The attributes of a statement.
pub(crate) fn statement_attributes(statement: &StatementNode) -> &[AttributeNode] {
    wgsl_parse::SyntaxNode::attributes(statement)
}
