//! The runner against shell scripts that stand in for the suite and the exit
//! programs.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use super::{Outcome, exit_programs, list_cases, run_case, run_exit_program, run_file};

/// An executable shell script in the directory.
fn script(directory: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = directory.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn a_file_runs_together_until_a_case_fails_and_its_later_cases_alone() {
    let directory = tempfile::tempdir().unwrap();
    // Unity's listing and results: a.c's second case fails, and c.c crashes.
    let suite = script(
        directory.path(),
        "suite",
        r#"[ -n "$MLN_FFI_TEST_FIXTURE_DIR" ] || exit 1
case "$1 $2" in
    "-l ") printf 'tests/native/%s\n' a.c:one a.c:two a.c:three c.c:crash ;;
    "-f tests/native/a.c")
        echo tests/native/a.c:1:one:PASS
        echo 'tests/native/a.c:2:two:FAIL: expected 1 was 2'
        echo tests/native/a.c:3:three:PASS ;;
    "-f tests/native/c.c") kill -SEGV $$ ;;
    "-n tests/native/a.c:three") echo tests/native/a.c:3:three:IGNORE ;;
esac"#,
    );
    let files = list_cases(&suite).unwrap();
    assert_eq!(files[0], ("a.c".to_owned(), vec!["one".to_owned(), "two".to_owned(), "three".to_owned()]));

    let (outcomes, alone) = run_file(&suite, "a.c", &files[0].1).unwrap();
    assert!(matches!(outcomes["a.c:one"], Outcome::Pass));
    assert!(matches!(&outcomes["a.c:two"], Outcome::Fail(message) if message == "expected 1 was 2"));
    assert_eq!(alone, ["a.c:three"]);
    assert!(matches!(run_case(&suite, "a.c:three").unwrap(), Outcome::Ignore));

    let (outcomes, _) = run_file(&suite, "c.c", &files[1].1).unwrap();
    assert!(
        matches!(&outcomes["c.c:crash"], Outcome::Fail(message) if message == "died of signal 11 before reporting")
    );
}

#[test]
fn exit_programs_pass_when_they_exit_with_status_0() {
    let directory = tempfile::tempdir().unwrap();
    script(directory.path(), "ok", "exit 0");
    script(directory.path(), "bad", "echo out; echo err >&2; exit 3");
    let programs = exit_programs(directory.path()).unwrap();
    assert_eq!(run_exit_program(&programs[0]), Err("exited with status 3: out; err".to_owned()));
    assert_eq!(run_exit_program(&programs[1]), Ok(()));
}
