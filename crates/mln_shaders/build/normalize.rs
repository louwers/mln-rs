//! Rewrites a WESL module so that each `@if`, `@elif` and `@else` is a
//! standalone `@if` of the whole condition its node is in, and no `@if`
//! decorates a compound statement.
//!
//! WESL leaves a flag that it keeps (`Feature::Keep`) in its `@if`s, but
//! inlines a compound statement it decorates whatever the condition, which
//! loses the condition. A compound statement's `@if` therefore moves onto
//! each statement in it, inlined as WESL inlines it when the condition
//! holds.

use wesl::syntax::{
    Attribute, AttributeNode, CompoundStatement, GlobalDeclaration, GlobalDeclarationNode, Statement, StatementNode,
    TranslationUnit,
};

use crate::cond::{Cond, TRUE};

/// `source` with its conditions standalone.
pub(crate) fn normalize(module: &str, source: &str) -> Result<String, String> {
    let mut unit: TranslationUnit = source.parse().map_err(|error| format!("{module}: {error}"))?;
    let fail = |error: String| format!("{module}: {error}");
    unit.imports =
        standalone(std::mem::take(&mut unit.imports), &TRUE, |import| Some(&mut import.attributes)).map_err(fail)?;
    unit.global_declarations = declarations(std::mem::take(&mut unit.global_declarations), &TRUE).map_err(fail)?;
    for declaration in &mut unit.global_declarations {
        match declaration.node_mut() {
            GlobalDeclaration::Struct(decl) => {
                decl.members = standalone(std::mem::take(&mut decl.members), &TRUE, |member| {
                    Some(&mut member.node_mut().attributes)
                })
                .map_err(fail)?;
            }
            GlobalDeclaration::Function(decl) => {
                decl.parameters = standalone(std::mem::take(&mut decl.parameters), &TRUE, |parameter| {
                    Some(&mut parameter.attributes)
                })
                .map_err(fail)?;
                compound(&mut decl.body).map_err(fail)?;
            }
            _ => {}
        }
    }
    Ok(unit.to_string())
}

/// The condition of each of `nodes`, a list whose `@if`, `@elif` and `@else`
/// chain, relative to the list: `None` for a node without one.
fn chain<N>(
    nodes: &mut [N],
    attributes: impl Fn(&mut N) -> Option<&mut Vec<AttributeNode>>,
) -> Result<Vec<Option<Cond>>, String> {
    // The condition under which no node of the chain so far is in, if a
    // chain is open.
    let mut rest: Option<Cond> = None;
    let mut conds = Vec::with_capacity(nodes.len());
    for node in nodes {
        let attribute = attributes(node).and_then(|attributes| {
            attributes.iter().find(|attribute| attribute.is_condcomp()).map(|attribute| attribute.node().clone())
        });
        let cond = match attribute {
            Some(Attribute::If(expression)) => {
                let cond = Cond::of(&expression)?;
                rest = Some(cond.clone().not());
                Some(cond)
            }
            Some(Attribute::Elif(expression)) => {
                let before = rest.take().ok_or("an @elif follows no @if")?;
                let cond = Cond::of(&expression)?;
                rest = Some(before.clone().and(cond.clone().not()));
                Some(before.and(cond))
            }
            Some(Attribute::Else) => Some(rest.take().ok_or("an @else follows no @if")?),
            _ => {
                rest = None;
                None
            }
        };
        conds.push(cond);
    }
    Ok(conds)
}

/// `attributes` with `cond` its only condition: none if it is true.
fn set_cond(attributes: &mut Vec<AttributeNode>, cond: &Cond) {
    attributes.retain(|attribute| !attribute.is_condcomp());
    if !cond.is_true() {
        attributes.insert(0, Attribute::If(cond.expression().into()).into());
    }
}

/// `nodes`, of a list that chains conditions, each with its whole condition
/// and `outer`'s, without those never in.
fn standalone<N>(
    mut nodes: Vec<N>,
    outer: &Cond,
    attributes: impl Fn(&mut N) -> Option<&mut Vec<AttributeNode>>,
) -> Result<Vec<N>, String> {
    let conds = chain(&mut nodes, &attributes)?;
    let mut kept = Vec::with_capacity(nodes.len());
    for (mut node, cond) in nodes.into_iter().zip(conds) {
        let cond = outer.clone().and(cond.unwrap_or(TRUE));
        if cond.is_false() {
            continue;
        }
        if let Some(attributes) = attributes(&mut node) {
            set_cond(attributes, &cond);
        } else if !cond.is_true() {
            return Err("a condition on a node that takes none".to_owned());
        }
        kept.push(node);
    }
    Ok(kept)
}

/// `nodes`, global declarations, standalone, with a compound declaration's
/// inlined.
fn declarations(mut nodes: Vec<GlobalDeclarationNode>, outer: &Cond) -> Result<Vec<GlobalDeclarationNode>, String> {
    let conds = chain(&mut nodes, declaration_attributes)?;
    let mut kept = Vec::with_capacity(nodes.len());
    for (mut node, cond) in nodes.into_iter().zip(conds) {
        let whole = outer.clone().and(cond.clone().unwrap_or(TRUE));
        if whole.is_false() || matches!(node.node(), GlobalDeclaration::Void) {
            continue;
        }
        if let GlobalDeclaration::Compound(compound) = node.node_mut() {
            kept.extend(declarations(std::mem::take(&mut compound.body), &whole)?);
            continue;
        }
        set_cond(declaration_attributes(&mut node).ok_or("a declaration without attributes")?, &whole);
        kept.push(node);
    }
    Ok(kept)
}

fn declaration_attributes(node: &mut GlobalDeclarationNode) -> Option<&mut Vec<AttributeNode>> {
    Some(match node.node_mut() {
        GlobalDeclaration::Void => return None,
        GlobalDeclaration::Declaration(decl) => &mut decl.attributes,
        GlobalDeclaration::TypeAlias(decl) => &mut decl.attributes,
        GlobalDeclaration::Struct(decl) => &mut decl.attributes,
        GlobalDeclaration::Function(decl) => &mut decl.attributes,
        GlobalDeclaration::ConstAssert(decl) => &mut decl.attributes,
        GlobalDeclaration::Compound(decl) => &mut decl.attributes,
    })
}

/// The attributes of a statement; none for an empty one.
pub(crate) fn statement_attributes(node: &mut StatementNode) -> Option<&mut Vec<AttributeNode>> {
    Some(match node.node_mut() {
        Statement::Void => return None,
        Statement::Compound(stmt) => &mut stmt.attributes,
        Statement::Assignment(stmt) => &mut stmt.attributes,
        Statement::Increment(stmt) => &mut stmt.attributes,
        Statement::Decrement(stmt) => &mut stmt.attributes,
        Statement::If(stmt) => &mut stmt.attributes,
        Statement::Switch(stmt) => &mut stmt.attributes,
        Statement::Loop(stmt) => &mut stmt.attributes,
        Statement::For(stmt) => &mut stmt.attributes,
        Statement::While(stmt) => &mut stmt.attributes,
        Statement::Break(stmt) => &mut stmt.attributes,
        Statement::Continue(stmt) => &mut stmt.attributes,
        Statement::Return(stmt) => &mut stmt.attributes,
        Statement::Discard(stmt) => &mut stmt.attributes,
        Statement::FunctionCall(stmt) => &mut stmt.attributes,
        Statement::ConstAssert(stmt) => &mut stmt.attributes,
        Statement::Declaration(stmt) => &mut stmt.attributes,
    })
}

/// The statements of `body`, standalone.
fn compound(body: &mut CompoundStatement) -> Result<(), String> {
    body.statements = statements(std::mem::take(&mut body.statements), &TRUE)?;
    Ok(())
}

/// `nodes`, statements, standalone with `outer`'s condition, each compound
/// statement that has a condition inlined into the list.
fn statements(mut nodes: Vec<StatementNode>, outer: &Cond) -> Result<Vec<StatementNode>, String> {
    let conds = chain(&mut nodes, statement_attributes)?;
    let mut kept = Vec::with_capacity(nodes.len());
    for (mut node, cond) in nodes.into_iter().zip(conds) {
        let whole = outer.clone().and(cond.clone().unwrap_or(TRUE));
        if whole.is_false() || matches!(node.node(), Statement::Void) {
            continue;
        }
        if let Statement::Compound(stmt) = node.node_mut() {
            if cond.is_some() {
                kept.extend(statements(std::mem::take(&mut stmt.statements), &whole)?);
                continue;
            }
            // A plain block keeps its scope, and its statements take the
            // condition.
            stmt.statements = statements(std::mem::take(&mut stmt.statements), &whole)?;
            kept.push(node);
            continue;
        }
        set_cond(statement_attributes(&mut node).expect("a statement with attributes"), &whole);
        inner(&mut node)?;
        kept.push(node);
    }
    Ok(kept)
}

/// Makes the statements inside `node` standalone.
fn inner(node: &mut StatementNode) -> Result<(), String> {
    match node.node_mut() {
        Statement::If(stmt) => {
            compound(&mut stmt.if_clause.body)?;
            for clause in &mut stmt.else_if_clauses {
                compound(&mut clause.body)?;
            }
            if let Some(clause) = &mut stmt.else_clause {
                compound(&mut clause.body)?;
            }
        }
        Statement::Switch(stmt) => {
            for clause in &mut stmt.clauses {
                compound(&mut clause.body)?;
            }
        }
        Statement::Loop(stmt) => {
            compound(&mut stmt.body)?;
            if let Some(continuing) = &mut stmt.continuing {
                compound(&mut continuing.body)?;
            }
        }
        Statement::For(stmt) => compound(&mut stmt.body)?,
        Statement::While(stmt) => compound(&mut stmt.body)?,
        _ => {}
    }
    Ok(())
}
