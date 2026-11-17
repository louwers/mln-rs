//! Conditions on feature flags: what an `@if` tests, and when a declaration
//! is reachable from an entry point.

use std::collections::BTreeSet;
use std::fmt;

use wesl::syntax::{BinaryOperator, Expression, LiteralExpression, UnaryOperator};

/// A boolean expression of feature flags, kept simplified: no constant in a
/// larger expression, no nested `And` in an `And` nor `Or` in an `Or`, no
/// repeated operand.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Cond {
    Const(bool),
    Flag(String),
    Not(Box<Cond>),
    And(Vec<Cond>),
    Or(Vec<Cond>),
}

pub(crate) const TRUE: Cond = Cond::Const(true);
pub(crate) const FALSE: Cond = Cond::Const(false);

/// With no more flags than this, a condition that is constant is folded to
/// its constant.
const MAX_FOLDED_FLAGS: usize = 16;

impl Cond {
    /// The condition of an `@if` expression.
    pub(crate) fn of(expression: &Expression) -> Result<Cond, String> {
        Ok(match expression {
            Expression::Literal(LiteralExpression::Bool(value)) => Cond::Const(*value),
            Expression::Parenthesized(paren) => Cond::of(&paren.expression)?,
            Expression::Unary(unary) if unary.operator == UnaryOperator::LogicalNegation => {
                Cond::of(&unary.operand)?.not()
            }
            Expression::Binary(binary) => {
                let (left, right) = (Cond::of(&binary.left)?, Cond::of(&binary.right)?);
                match binary.operator {
                    BinaryOperator::ShortCircuitAnd => left.and(right),
                    BinaryOperator::ShortCircuitOr => left.or(right),
                    _ => return Err(format!("{expression} is no condition")),
                }
            }
            Expression::TypeOrIdentifier(ty) if ty.template_args.is_none() && ty.path.is_none() => {
                Cond::Flag(ty.ident.name().clone())
            }
            _ => return Err(format!("{expression} is no condition")),
        })
    }

    /// The expression of an `@if` of this condition.
    pub(crate) fn expression(&self) -> Expression {
        self.to_string().parse().expect("a condition prints as an expression")
    }

    pub(crate) fn is_true(&self) -> bool {
        *self == TRUE
    }

    pub(crate) fn is_false(&self) -> bool {
        *self == FALSE
    }

    #[must_use]
    pub(crate) fn not(self) -> Cond {
        match self {
            Cond::Const(value) => Cond::Const(!value),
            Cond::Not(inner) => *inner,
            cond => Cond::Not(Box::new(cond)),
        }
        .folded()
    }

    #[must_use]
    pub(crate) fn and(self, other: Cond) -> Cond {
        Cond::all([self, other])
    }

    #[must_use]
    pub(crate) fn or(self, other: Cond) -> Cond {
        Cond::any([self, other])
    }

    /// All of `conds`; true if there are none.
    pub(crate) fn all(conds: impl IntoIterator<Item = Cond>) -> Cond {
        Cond::join(conds, true)
    }

    /// Any of `conds`; false if there are none.
    pub(crate) fn any(conds: impl IntoIterator<Item = Cond>) -> Cond {
        Cond::join(conds, false)
    }

    /// `conds` joined by `And` if `and`, else by `Or`.
    fn join(conds: impl IntoIterator<Item = Cond>, and: bool) -> Cond {
        // `and`'s identity, and the constant that decides it.
        let (identity, absorbing) = (Cond::Const(and), Cond::Const(!and));
        let mut operands = BTreeSet::new();
        for cond in conds {
            match cond {
                cond if cond == identity => {}
                cond if cond == absorbing => return absorbing,
                Cond::And(inner) if and => operands.extend(inner),
                Cond::Or(inner) if !and => operands.extend(inner),
                cond => {
                    operands.insert(cond);
                }
            }
        }
        let mut operands: Vec<Cond> = operands.into_iter().collect();
        match operands.len() {
            0 => identity,
            1 => operands.pop().expect("one operand"),
            _ if and => Cond::And(operands).folded(),
            _ => Cond::Or(operands).folded(),
        }
    }

    /// The flags the condition tests.
    pub(crate) fn flags(&self) -> BTreeSet<&str> {
        let mut flags = BTreeSet::new();
        self.collect_flags(&mut flags);
        flags
    }

    fn collect_flags<'a>(&'a self, flags: &mut BTreeSet<&'a str>) {
        match self {
            Cond::Const(_) => {}
            Cond::Flag(flag) => {
                flags.insert(flag);
            }
            Cond::Not(inner) => inner.collect_flags(flags),
            Cond::And(conds) | Cond::Or(conds) => conds.iter().for_each(|cond| cond.collect_flags(flags)),
        }
    }

    /// The condition's value with the flags `on` on.
    pub(crate) fn eval(&self, on: &dyn Fn(&str) -> bool) -> bool {
        match self {
            Cond::Const(value) => *value,
            Cond::Flag(flag) => on(flag),
            Cond::Not(inner) => !inner.eval(on),
            Cond::And(conds) => conds.iter().all(|cond| cond.eval(on)),
            Cond::Or(conds) => conds.iter().any(|cond| cond.eval(on)),
        }
    }

    /// The constant the condition always is, if it is one and tests few
    /// enough flags to tell; else itself.
    fn folded(self) -> Cond {
        let flags: Vec<String> = self.flags().into_iter().map(str::to_owned).collect();
        if flags.len() > MAX_FOLDED_FLAGS {
            return self;
        }
        let value =
            |mask: u32| self.eval(&|flag| flags.iter().position(|f| f == flag).is_some_and(|i| mask & (1 << i) != 0));
        let first = value(0);
        if (1..1u32 << flags.len()).all(|mask| value(mask) == first) { Cond::Const(first) } else { self }
    }
}

impl fmt::Display for Cond {
    /// The condition in the syntax of the preprocessor's `#if`, and of
    /// WESL's `@if`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operand = |f: &mut fmt::Formatter<'_>, cond: &Cond| match cond {
            Cond::And(_) | Cond::Or(_) => write!(f, "({cond})"),
            _ => write!(f, "{cond}"),
        };
        match self {
            Cond::Const(value) => write!(f, "{value}"),
            Cond::Flag(flag) => f.write_str(flag),
            Cond::Not(inner) => {
                f.write_str("!")?;
                operand(f, inner)
            }
            Cond::And(conds) | Cond::Or(conds) => {
                let separator = if matches!(self, Cond::And(_)) { " && " } else { " || " };
                for (index, cond) in conds.iter().enumerate() {
                    if index > 0 {
                        f.write_str(separator)?;
                    }
                    operand(f, cond)?;
                }
                Ok(())
            }
        }
    }
}
