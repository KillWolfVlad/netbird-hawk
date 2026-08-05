use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::Duration,
};

use netbird_hawk::netbird::{NetbirdApi, NetbirdClient, TokioCommandRunner};
use predicates::prelude::*;
use tempfile::TempDir;

struct Harness {
    root: TempDir,
    hawk: PathBuf,
    fake_netbird: PathBuf,
    state: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let fake_netbird = root.path().join(if cfg!(windows) {
            "netbird.exe"
        } else {
            "netbird"
        });
        let compilation = Command::new("rustc")
            .args(["--edition=2024", "tests/fixtures/fake_netbird.rs", "-o"])
            .arg(&fake_netbird)
            .output()
            .unwrap();
        assert!(
            compilation.status.success(),
            "fake compilation failed: {}",
            String::from_utf8_lossy(&compilation.stderr)
        );
        let state = root.path().join("active-profile");
        fs::write(&state, "alpha").unwrap();
        Self {
            root,
            hawk: assert_cmd::cargo::cargo_bin!("netbird-hawk").to_path_buf(),
            fake_netbird,
            state,
        }
    }

    fn command(&self) -> Command {
        let existing_path = std::env::var_os("PATH").unwrap_or_default();
        let joined = std::env::join_paths(
            std::iter::once(self.root.path().to_path_buf())
                .chain(std::env::split_paths(&existing_path)),
        )
        .unwrap();
        let mut command = Command::new(&self.hawk);
        command
            .env("PATH", joined)
            .env("NETBIRD_HAWK_HOME", self.root.path())
            .env("FAKE_NETBIRD_STATE", &self.state);
        command
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.command().args(arguments).output().unwrap()
    }

    fn set_behavior(&self, behavior: &str) {
        fs::write(self.state.with_extension("behavior"), behavior).unwrap();
    }

    fn assert_success(output: &Output) {
        assert!(
            output.status.success(),
            "command failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.command().arg("stop").output();
    }
}

#[test]
fn fake_executable_supports_status_selection_failure_and_timeout_fixtures() {
    let harness = Harness::new();
    let status = Command::new(&harness.fake_netbird)
        .arg("status")
        .env("FAKE_NETBIRD_STATE", &harness.state)
        .output()
        .unwrap();
    Harness::assert_success(&status);
    assert!(
        predicate::str::contains("Profile: alpha").eval(&String::from_utf8_lossy(&status.stdout))
    );

    let selected = Command::new(&harness.fake_netbird)
        .args(["profile", "select", "beta"])
        .env("FAKE_NETBIRD_STATE", &harness.state)
        .output()
        .unwrap();
    Harness::assert_success(&selected);
    assert_eq!(fs::read_to_string(&harness.state).unwrap(), "beta");

    harness.set_behavior("status-failure");
    let failure = Command::new(&harness.fake_netbird)
        .arg("status")
        .env("FAKE_NETBIRD_STATE", &harness.state)
        .output()
        .unwrap();
    assert_eq!(failure.status.code(), Some(9));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let client = NetbirdClient::new(
        harness.fake_netbird.clone(),
        TokioCommandRunner,
        Duration::from_millis(75),
    )
    .unwrap();
    harness.set_behavior("select-failure");
    assert!(runtime.block_on(client.select_profile("beta")).is_err());
    harness.set_behavior("select-timeout");
    let started = std::time::Instant::now();
    assert!(runtime.block_on(client.select_profile("beta")).is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn detached_lifecycle_is_ready_reconfigurable_concurrent_and_stoppable() {
    let harness = Harness::new();
    let start = harness.run(&[
        "start",
        "--profile",
        "alpha",
        "-p",
        "beta",
        "--time",
        "8:24",
    ]);
    Harness::assert_success(&start);
    assert!(String::from_utf8_lossy(&start.stdout).contains("daemon started"));

    let status = harness.run(&["status"]);
    Harness::assert_success(&status);
    let status_text = String::from_utf8_lossy(&status.stdout);
    assert!(status_text.contains("state: running"));
    assert!(status_text.contains("profiles: alpha -> beta"));
    assert!(status_text.contains("active profile: alpha"));

    let mut first = harness.command();
    first
        .args([
            "start", "-p", "alpha", "-p", "beta", "-p", "gamma", "-t", "9:31",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut second = harness.command();
    second
        .args(["start", "-p", "gamma", "-p", "alpha", "-t", "10:32"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let first = first.spawn().unwrap();
    let second = second.spawn().unwrap();
    Harness::assert_success(&first.wait_with_output().unwrap());
    Harness::assert_success(&second.wait_with_output().unwrap());

    let status = harness.run(&["status"]);
    Harness::assert_success(&status);
    assert!(String::from_utf8_lossy(&status.stdout).contains("state: running"));

    let stop = harness.run(&["stop"]);
    Harness::assert_success(&stop);
    assert!(String::from_utf8_lossy(&stop.stdout).contains("daemon stopped"));
    let repeated = harness.run(&["stop"]);
    Harness::assert_success(&repeated);
    assert!(String::from_utf8_lossy(&repeated.stdout).contains("already stopped"));
}

#[test]
fn invalid_reconfiguration_preserves_the_running_generation() {
    let harness = Harness::new();
    Harness::assert_success(&harness.run(&["start", "-p", "alpha", "-p", "beta", "-t", "8:24"]));
    harness.set_behavior("malformed-status");
    let rejected = harness.run(&["start", "-p", "replacement", "-p", "other", "-t", "9:00"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("Profile field was missing"));

    harness.set_behavior("ok");
    let status = harness.run(&["status"]);
    Harness::assert_success(&status);
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(status.contains("profiles: alpha -> beta"));
    assert!(!status.contains("replacement"));
}
