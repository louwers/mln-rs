use expression_test::{find_metrics_dir, print_stats, run};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut names = Vec::new();
    let mut filter = None;
    let mut write_expected = None;
    let mut verbose = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-f" | "--filter" => filter = args.next(),
            "-v" | "--verbose" => verbose = true,
            "--write-expected-failures" => write_expected = args.next(),
            _ => names.push(arg),
        }
    }

    let Some(metrics) = find_metrics_dir() else {
        eprintln!("Test path 'metrics/integration/expression-tests' does not exist.");
        std::process::exit(3);
    };
    let stats = run(&metrics, &names, filter.as_deref(), verbose);
    print_stats(&stats);

    if let Some(path) = write_expected {
        let mut ids: Vec<&str> = stats.failed.iter().chain(&stats.errored).map(|t| t.id.as_str()).collect();
        ids.sort_unstable();
        let mut text = String::new();
        for id in ids {
            text.push_str(id);
            text.push('\n');
        }
        std::fs::write(&path, text).expect("write expected failures");
    }

    std::process::exit(i32::from(stats.errored.len() + stats.failed.len() != 0));
}
