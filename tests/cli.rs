use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn root_help_works_with_flag_and_command_on_stdout() {
    for argument in ["--help", "help"] {
        Command::cargo_bin("netbird-hawk")
            .unwrap()
            .arg(argument)
            .assert()
            .success()
            .stdout(predicate::str::contains("start"))
            .stdout(predicate::str::contains("stop"))
            .stdout(predicate::str::contains("status"))
            .stderr(predicate::str::is_empty());
    }
}

#[test]
fn subcommand_help_documents_flags() {
    Command::cargo_bin("netbird-hawk")
        .unwrap()
        .args(["start", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--profile"))
        .stdout(predicate::str::contains("--time"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn validation_failure_is_nonzero_and_uses_stderr() {
    Command::cargo_bin("netbird-hawk")
        .unwrap()
        .args(["start", "--profile", "alpha", "--time", "25:00"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn stopped_status_is_successful_and_uses_stdout() {
    let root = tempfile::tempdir().unwrap();
    Command::cargo_bin("netbird-hawk")
        .unwrap()
        .env("NETBIRD_HAWK_HOME", root.path())
        .arg("status")
        .assert()
        .success()
        .stdout("state: stopped\n")
        .stderr(predicate::str::is_empty());
}
