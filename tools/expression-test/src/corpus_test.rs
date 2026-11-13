// Runs the whole expression corpus. The test fails only on regressions:
// failures not listed in expected_failures.txt, which names a whole test
// (`to-color/oklch`) or one of its inputs by index (`to-color/oklch#3`).
// Listed entries that pass are reported so the list can shrink.

use std::collections::BTreeSet;

use expression_test::runner::TestRunOutput;
use expression_test::{find_metrics_dir, print_stats, run};

/// The entries of `expected_failures.txt` that `t` fails: the whole test, or
/// the inputs that evaluate differently when only inputs do.
fn failures(t: &TestRunOutput) -> Vec<String> {
    if t.failed_inputs.is_empty() {
        vec![t.id.clone()]
    } else {
        t.failed_inputs.iter().map(|i| format!("{}#{i}", t.id)).collect()
    }
}

#[test]
fn expression_corpus() {
    let metrics = find_metrics_dir().expect("the C++ repository's metrics/integration/expression-tests");
    let stats = run(&metrics, &[], None, false);
    print_stats(&stats);

    let expected_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("expected_failures.txt");
    let expected_text = std::fs::read_to_string(&expected_path).expect("expected_failures.txt");
    let expected: BTreeSet<&str> =
        expected_text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();

    let mut regressions = Vec::new();
    let mut failing = BTreeSet::new();
    for t in stats.failed.iter().chain(&stats.errored) {
        let unexpected: Vec<String> = failures(t)
            .into_iter()
            .filter(|f| !expected.contains(t.id.as_str()) && !expected.contains(f.as_str()))
            .collect();
        if !unexpected.is_empty() {
            regressions.push((unexpected, t));
        }
        failing.insert(t.id.clone());
        failing.extend(failures(t));
    }
    let checked: BTreeSet<&str> =
        stats.passed.iter().chain(&stats.failed).chain(&stats.errored).map(|t| t.id.as_str()).collect();
    let newly_passing: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|e| checked.contains(e.split_once('#').map_or(*e, |(id, _)| id)) && !failing.contains(*e))
        .collect();

    println!(
        "expression corpus: {} of {} passed ({} ignored, {} failed, {} errored); {} expected failures",
        stats.passed.len() + stats.ignore_passed.len(),
        stats.test_count(),
        stats.ignore_failed.len() + stats.ignore_passed.len(),
        stats.failed.len(),
        stats.errored.len(),
        expected.len()
    );
    for id in &newly_passing {
        println!("now passing (remove from expected_failures.txt): {id}");
    }
    for (unexpected, t) in &regressions {
        println!("* REGRESSION {}\n{}", unexpected.join(", "), t.text);
    }
    assert!(regressions.is_empty(), "{} expression tests regressed", regressions.len());
}
