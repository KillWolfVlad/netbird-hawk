use std::{
    fs,
    io::{self, Read},
    path::PathBuf,
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use netbird_hawk::netbird::{NetbirdApi, NetbirdClient, TokioCommandRunner};
use predicates::prelude::*;
use tempfile::TempDir;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn bounded_output(mut command: Command, timeout: Duration) -> Result<Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let description = format!("{command:?}");
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn {description}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("stdout was not captured for {description}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("stderr was not captured for {description}"))?;
    let stdout = read_stream(stdout);
    let stderr = read_stream(stderr);

    let status = wait_for_exit(&mut child, &description, timeout)?;
    let capture_deadline = Instant::now() + CAPTURE_TIMEOUT;
    let stdout = receive_stream(stdout, capture_deadline, "stdout", &description)?;
    let stderr = receive_stream(stderr, capture_deadline, "stderr", &description)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn bounded_status(mut command: Command, timeout: Duration) -> Result<ExitStatus, String> {
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let description = format!("{command:?}");
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn {description}: {error}"))?;
    wait_for_exit(&mut child, &description, timeout)
}

fn wait_for_exit(
    child: &mut Child,
    description: &str,
    timeout: Duration,
) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let kill_result = child.kill();
                let wait_result = child.wait();
                return Err(format!(
                    "command timed out after {timeout:?}: {description}; kill={kill_result:?}; wait={wait_result:?}"
                ));
            }
            Err(error) => {
                return Err(format!("failed while waiting for {description}: {error}"));
            }
        }
    }
}

fn read_stream(mut stream: impl Read + Send + 'static) -> Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stream.read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });
    receiver
}

fn receive_stream(
    receiver: Receiver<io::Result<Vec<u8>>>,
    deadline: Instant,
    stream_name: &str,
    description: &str,
) -> Result<Vec<u8>, String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    receiver
        .recv_timeout(remaining)
        .map_err(|error| {
            format!(
                "{stream_name} did not reach EOF within {CAPTURE_TIMEOUT:?} after {description} exited: {error}"
            )
        })?
        .map_err(|error| format!("failed to read {stream_name} from {description}: {error}"))
}

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
        let mut compilation = Command::new("rustc");
        compilation
            .args(["--edition=2024", "tests/fixtures/fake_netbird.rs", "-o"])
            .arg(&fake_netbird);
        let compilation = bounded_output(compilation, COMMAND_TIMEOUT).unwrap();
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
        let mut command = self.command();
        command.args(arguments);
        self.complete(command)
    }

    fn complete(&self, command: Command) -> Output {
        bounded_output(command, COMMAND_TIMEOUT).unwrap_or_else(|error| {
            let cleanup = self.cleanup_worker();
            panic!(
                "{error}\ncleanup result: {cleanup:?}\n{}",
                self.diagnostics()
            );
        })
    }

    fn cleanup_worker(&self) -> Result<ExitStatus, String> {
        let mut command = self.command();
        command.arg("stop");
        bounded_status(command, CLEANUP_TIMEOUT)
    }

    fn diagnostics(&self) -> String {
        let mut files = vec![
            self.root.path().join("config/config.json"),
            self.root.path().join("state/control.json"),
            self.root.path().join("state/runtime.json"),
            self.root.path().join("state/journal.json"),
        ];
        if let Ok(entries) = fs::read_dir(self.root.path().join("logs")) {
            files.extend(entries.flatten().map(|entry| entry.path()));
        }
        files.sort();

        let mut diagnostics = Vec::new();
        for path in files {
            if let Ok(bytes) = fs::read(&path) {
                let tail = bytes.len().saturating_sub(16 * 1024);
                diagnostics.push(format!(
                    "{}:\n{}",
                    path.display(),
                    String::from_utf8_lossy(&bytes[tail..])
                ));
            }
        }
        if diagnostics.is_empty() {
            "no state or log diagnostics were available".to_owned()
        } else {
            diagnostics.join("\n")
        }
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
        let _ = self.cleanup_worker();
    }
}

#[test]
fn fake_executable_supports_status_selection_failure_and_timeout_fixtures() {
    let harness = Harness::new();
    let mut status_command = Command::new(&harness.fake_netbird);
    status_command
        .arg("status")
        .env("FAKE_NETBIRD_STATE", &harness.state);
    let status = harness.complete(status_command);
    Harness::assert_success(&status);
    assert!(
        predicate::str::contains("Profile: alpha").eval(&String::from_utf8_lossy(&status.stdout))
    );

    let mut selected_command = Command::new(&harness.fake_netbird);
    selected_command
        .args(["profile", "select", "beta"])
        .env("FAKE_NETBIRD_STATE", &harness.state);
    let selected = harness.complete(selected_command);
    Harness::assert_success(&selected);
    assert_eq!(fs::read_to_string(&harness.state).unwrap(), "beta");

    harness.set_behavior("status-failure");
    let mut failure_command = Command::new(&harness.fake_netbird);
    failure_command
        .arg("status")
        .env("FAKE_NETBIRD_STATE", &harness.state);
    let failure = harness.complete(failure_command);
    assert_eq!(failure.status.code(), Some(9));

    harness.set_behavior("status-timeout");
    let mut timeout_command = Command::new(&harness.fake_netbird);
    timeout_command
        .arg("status")
        .env("FAKE_NETBIRD_STATE", &harness.state);
    let started = Instant::now();
    assert!(
        bounded_output(timeout_command, Duration::from_millis(75))
            .unwrap_err()
            .contains("timed out")
    );
    assert!(started.elapsed() < Duration::from_secs(2));

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

    let (first, second) = thread::scope(|scope| {
        let first = scope.spawn(|| {
            harness.run(&[
                "start", "-p", "alpha", "-p", "beta", "-p", "gamma", "-t", "9:31",
            ])
        });
        let second =
            scope.spawn(|| harness.run(&["start", "-p", "gamma", "-p", "alpha", "-t", "10:32"]));
        (first.join().unwrap(), second.join().unwrap())
    });
    Harness::assert_success(&first);
    Harness::assert_success(&second);

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

#[cfg(windows)]
#[test]
fn windows_captured_start_reaches_eof_while_worker_remains_live() {
    let harness = Harness::new();
    let start = harness.run(&["start", "-p", "alpha", "-p", "beta", "-t", "8:24"]);
    Harness::assert_success(&start);
    assert!(String::from_utf8_lossy(&start.stdout).contains("daemon started"));

    // bounded_output returning proves both captured streams reached EOF even
    // though the detached worker still owns the lifetime lock.
    let status = harness.run(&["status"]);
    Harness::assert_success(&status);
    assert!(String::from_utf8_lossy(&status.stdout).contains("state: running"));

    let stop = harness.run(&["stop"]);
    Harness::assert_success(&stop);
    assert!(String::from_utf8_lossy(&stop.stdout).contains("daemon stopped"));
}
