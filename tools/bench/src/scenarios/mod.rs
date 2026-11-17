//! The registry of scenarios.

pub(crate) mod r#macro;
pub(crate) mod micro;

use crate::scenario::Scenario;

/// Every scenario, in the order `list` and `run` show them.
pub(crate) fn all() -> impl Iterator<Item = &'static Scenario> {
    [
        micro::style::SCENARIOS,
        micro::expression::SCENARIOS,
        micro::decode::SCENARIOS,
        micro::geojson::SCENARIOS,
        micro::text::SCENARIOS,
        micro::layout::SCENARIOS,
        micro::symbols::SCENARIOS,
        micro::feature_state::SCENARIOS,
        micro::worker_codec::SCENARIOS,
        micro::run_loop::SCENARIOS,
        micro::runtime::SCENARIOS,
        r#macro::load::SCENARIOS,
        r#macro::frame::SCENARIOS,
        r#macro::navigate::SCENARIOS,
        r#macro::update::SCENARIOS,
        r#macro::tile_server::SCENARIOS,
    ]
    .into_iter()
    .flatten()
}

/// The scenario named `name`.
pub(crate) fn find(name: &str) -> Option<&'static Scenario> {
    all().find(|scenario| scenario.name == name)
}

/// The scenarios whose names contain one of `filters`, or all for none.
pub(crate) fn select(filters: &[String]) -> Vec<&'static Scenario> {
    all().filter(|scenario| filters.is_empty() || filters.iter().any(|filter| scenario.name.contains(filter))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_paths_under_their_kind() {
        let mut names = std::collections::BTreeSet::new();
        for scenario in all() {
            let name = scenario.name;
            assert!(names.insert(name), "{name} twice");
            let segments: Vec<&str> = name.split('/').collect();
            assert!(segments.len() >= 3, "{name}: kind/subject/case");
            let kind = serde_json::to_value(scenario.kind).unwrap();
            assert_eq!(segments[0], kind, "{name}: its kind first");
            for segment in segments {
                assert!(
                    !segment.is_empty()
                        && segment.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                    "{name}: segments of a-z, 0-9 and -"
                );
            }
            assert!(scenario.question.ends_with('?'), "{name}: its question");
        }
    }

    #[test]
    fn filters_match_parts_of_names() {
        assert!(select(&["micro/".to_owned()]).iter().all(|scenario| scenario.name.starts_with("micro/")));
        assert_eq!(select(&[]).len(), all().count());
    }
}
