//! The feature state that a map keeps for its sources' features.

use feature::{Identifier, ValueObject};
use mln_util::error::Error;

/// The state of one feature: keys and their values.
pub type FeatureState = ValueObject;

/// The features whose state to set, read or remove.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureStateSelector {
    pub source_id: String,
    /// The source layer, for a vector source. `None`, like an empty string,
    /// names no source layer.
    pub source_layer_id: Option<String>,
    /// The feature. Setting and reading state need it. Without it, a removal
    /// removes the state of every feature in the source layer.
    ///
    /// The map compares IDs as text, so a number names the same feature as
    /// the string of its shortest decimal form: 7, 7.0 and "7" name one
    /// feature.
    pub feature_id: Option<Identifier>,
    /// The one key that a removal removes. It needs `feature_id`. `None`,
    /// like an empty string, removes every key.
    pub state_key: Option<String>,
}

impl FeatureStateSelector {
    /// Selects the feature `feature_id` of the source `source_id`.
    pub fn feature(source_id: impl Into<String>, feature_id: Identifier) -> Self {
        FeatureStateSelector {
            source_id: source_id.into(),
            source_layer_id: None,
            feature_id: Some(feature_id),
            state_key: None,
        }
    }

    #[must_use]
    pub fn with_source_layer(mut self, source_layer_id: impl Into<String>) -> Self {
        self.source_layer_id = Some(source_layer_id.into());
        self
    }

    #[must_use]
    pub fn with_state_key(mut self, state_key: impl Into<String>) -> Self {
        self.state_key = Some(state_key.into());
        self
    }

    /// The selector's keys in the store, checked: `needs_feature` for a set
    /// or a read.
    pub(crate) fn key(&self, needs_feature: bool) -> Result<SelectorKey, Error> {
        if self.source_id.is_empty() {
            return Err(Error::invalid_argument("feature state source_id must not be empty"));
        }
        let feature_id = match &self.feature_id {
            None => None,
            Some(Identifier::Null) => {
                return Err(Error::invalid_argument("feature state feature_id must not be null"));
            }
            Some(Identifier::Double(value)) if !value.is_finite() => {
                return Err(Error::invalid_argument("feature_id double_value must be finite"));
            }
            Some(id) => mln_util::feature::feature_id_to_string(id),
        };
        let state_key = self.state_key.clone().filter(|key| !key.is_empty());
        if feature_id.is_none() {
            if needs_feature {
                return Err(Error::invalid_argument("feature state selector requires feature_id"));
            }
            if state_key.is_some() {
                return Err(Error::invalid_argument("feature state selector state_key requires feature_id"));
            }
        }
        Ok(SelectorKey {
            source_id: self.source_id.clone(),
            source_layer_id: self.source_layer_id.clone().unwrap_or_default(),
            feature_id,
            state_key,
        })
    }
}

/// A checked selector: the empty source layer stands for none.
#[derive(Clone, Debug)]
pub(crate) struct SelectorKey {
    pub(crate) source_id: String,
    pub(crate) source_layer_id: String,
    pub(crate) feature_id: Option<String>,
    pub(crate) state_key: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(id: &str) -> Identifier {
        Identifier::String(id.to_owned())
    }

    fn feature_text(id: Identifier) -> String {
        FeatureStateSelector::feature("points", id).key(true).unwrap().feature_id.unwrap()
    }

    #[test]
    fn numbers_name_the_feature_of_their_text() {
        for id in [Identifier::Uint(7), Identifier::Int(7), Identifier::Double(7.0), text("7")] {
            assert_eq!("7", feature_text(id));
        }
        assert_eq!("-3", feature_text(Identifier::Int(-3)));
        assert_eq!("0.1", feature_text(Identifier::Double(0.1)));
        assert_eq!("1e21", feature_text(Identifier::Double(1e21)));
        // The empty source layer and state key are none.
        let key =
            FeatureStateSelector::feature("points", text("a")).with_source_layer("").with_state_key("").key(false);
        let key = key.unwrap();
        assert_eq!(("", None), (key.source_layer_id.as_str(), key.state_key));
    }

    #[test]
    fn selectors_check_what_each_use_needs() {
        let message = |selector: FeatureStateSelector, needs_feature| selector.key(needs_feature).unwrap_err().message;
        let feature = FeatureStateSelector::feature("points", text("a"));
        assert_eq!(
            "feature state source_id must not be empty",
            message(FeatureStateSelector { source_id: String::new(), ..feature.clone() }, false)
        );
        let source = FeatureStateSelector { feature_id: None, ..feature.clone() };
        assert_eq!("feature state selector requires feature_id", message(source.clone(), true));
        assert_eq!("feature state selector state_key requires feature_id", message(source.with_state_key("k"), false));
        assert!(FeatureStateSelector { feature_id: None, ..feature }.with_state_key("").key(false).is_ok());
        assert_eq!(
            "feature_id double_value must be finite",
            message(FeatureStateSelector::feature("points", Identifier::Double(f64::NAN)), true)
        );
    }
}
