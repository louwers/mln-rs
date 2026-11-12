//! `style-codegen --spec <v8.json> --out <crates/mln_style/src> [--check]`
//! writes (or with `--check` compares) the generated style code.
//!
//! `just generate-style-code` regenerates the checked-in files.

use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage: style-codegen --spec <v8.json> --out <dir> [--check]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut spec = None;
    let mut out = None;
    let mut check = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--spec" => spec = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--check" => check = true,
            _ => return usage(),
        }
    }
    let Some(spec) = spec else { return usage() };
    let spec_text = match std::fs::read_to_string(&spec) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("{}: {e}", spec.display());
            return ExitCode::FAILURE;
        }
    };

    let Some(out) = out else { return usage() };
    if check {
        return match style_codegen::check(&spec_text, &out) {
            Ok(stale) if stale.is_empty() => ExitCode::SUCCESS,
            Ok(stale) => {
                for path in stale {
                    eprintln!("* outdated: {}", out.join(path).display());
                }
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        };
    }
    let files = match style_codegen::generate(&spec_text) {
        Ok(files) => files,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    for file in files {
        let path = out.join(&file.path);
        // writeIfModified
        if std::fs::read_to_string(&path).ok().as_deref() == Some(file.content.as_str()) {
            eprintln!("* Skipping file '{}' because it is up-to-date", path.display());
            continue;
        }
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("{}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
        if let Err(e) = std::fs::write(&path, &file.content) {
            eprintln!("{}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        eprintln!("* Updating outdated file '{}'", path.display());
    }
    ExitCode::SUCCESS
}
