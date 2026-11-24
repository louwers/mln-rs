//! The expression forms: the nodes that parse their own arguments, one
//! module per kind of node.

pub(crate) mod assertion;
pub(crate) mod at;
pub(crate) mod boolean_operator;
pub(crate) mod case;
pub(crate) mod coalesce;
pub(crate) mod coercion;
pub(crate) mod collator;
pub(crate) mod comparison;
pub(crate) mod compound;
pub(crate) mod distance;
pub(crate) mod format;
pub(crate) mod format_section_override;
pub(crate) mod image;
pub(crate) mod in_;
pub(crate) mod index_of;
pub(crate) mod interpolate;
pub(crate) mod length;
pub(crate) mod let_;
pub(crate) mod literal;
pub(crate) mod match_;
pub(crate) mod number_format;
pub(crate) mod semiliteral;
pub(crate) mod slice;
pub(crate) mod step;
pub(crate) mod within;
