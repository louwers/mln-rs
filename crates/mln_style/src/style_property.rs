use feature::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    #[default]
    Undefined,
    Constant,
    Expression,
    Transition,
}

/// Generic representation of a style property.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleProperty {
    value: Value,
    kind: Kind,
}

impl StyleProperty {
    #[must_use]
    pub fn new(value: Value, kind: Kind) -> Self {
        StyleProperty { value, kind }
    }

    #[must_use]
    pub fn get_value(&self) -> &Value {
        &self.value
    }

    pub fn get_value_mut(&mut self) -> &mut Value {
        &mut self.value
    }

    #[must_use]
    pub fn get_kind(&self) -> Kind {
        self.kind
    }
}
