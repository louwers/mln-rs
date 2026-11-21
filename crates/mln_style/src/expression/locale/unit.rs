//! The units of `number-format`: those `Intl.NumberFormat` takes.

use std::fmt;

/// The simple units ECMA-402 sanctions, in sorted order.
const SIMPLE_UNITS: [&str; 45] = [
    "acre",
    "bit",
    "byte",
    "celsius",
    "centimeter",
    "day",
    "degree",
    "fahrenheit",
    "fluid-ounce",
    "foot",
    "gallon",
    "gigabit",
    "gigabyte",
    "gram",
    "hectare",
    "hour",
    "inch",
    "kilobit",
    "kilobyte",
    "kilogram",
    "kilometer",
    "liter",
    "megabit",
    "megabyte",
    "meter",
    "microsecond",
    "mile",
    "mile-scandinavian",
    "milliliter",
    "millimeter",
    "millisecond",
    "minute",
    "month",
    "nanosecond",
    "ounce",
    "percent",
    "petabyte",
    "pound",
    "second",
    "stone",
    "terabit",
    "terabyte",
    "week",
    "yard",
    "year",
];

fn simple_unit(identifier: &str) -> Option<&'static str> {
    SIMPLE_UNITS.binary_search(&identifier).ok().map(|index| SIMPLE_UNITS[index])
}

/// A unit `Intl.NumberFormat` takes: one of ECMA-402's sanctioned simple
/// units, such as `meter`, or one per another, such as `kilometer-per-hour`.
/// It displays as the identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Unit {
    /// The unit, or the numerator of a compound unit.
    pub(crate) unit: &'static str,
    /// The denominator of a compound unit.
    pub(crate) per: Option<&'static str>,
}

impl Unit {
    /// The unit an identifier names. Identifiers are case-sensitive. Empty
    /// means none.
    ///
    /// # Errors
    ///
    /// When the identifier isn't a sanctioned simple unit or one per another.
    pub(crate) fn parse(identifier: &str) -> Result<Option<Unit>, String> {
        if identifier.is_empty() {
            return Ok(None);
        }
        let unit = match identifier.split_once("-per-") {
            None => simple_unit(identifier).map(|unit| Unit { unit, per: None }),
            Some((unit, per)) => {
                simple_unit(unit).zip(simple_unit(per)).map(|(unit, per)| Unit { unit, per: Some(per) })
            }
        };
        unit.map(Some).ok_or_else(|| format!("{identifier} is not a unit that number-format supports"))
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.unit)?;
        if let Some(per) = self.per {
            write!(f, "-per-{per}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_units_are_sorted() {
        assert!(SIMPLE_UNITS.is_sorted());
    }

    #[test]
    fn units_are_sanctioned_simple_units_or_one_per_another() {
        assert_eq!(Unit::parse(""), Ok(None));
        assert_eq!(Unit::parse("meter"), Ok(Some(Unit { unit: "meter", per: None })));
        assert_eq!(Unit::parse("mile-scandinavian"), Ok(Some(Unit { unit: "mile-scandinavian", per: None })));
        assert_eq!(Unit::parse("kilometer-per-hour"), Ok(Some(Unit { unit: "kilometer", per: Some("hour") })));
        for identifier in ["Meter", "square-meter", "meter-per", "per-meter", "kilometer-per-hour-per-second"] {
            assert!(Unit::parse(identifier).is_err(), "{identifier}");
        }
    }

    #[test]
    fn units_display_as_their_identifiers() {
        for identifier in ["percent", "liter-per-kilometer", "mile-scandinavian-per-hour"] {
            assert_eq!(Unit::parse(identifier).unwrap().unwrap().to_string(), identifier);
        }
    }
}
