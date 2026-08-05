use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitStatus,
    time::Duration,
};

use async_trait::async_trait;
use tokio::process::Command;

use crate::{
    error::{HawkError, Result},
    model::SanitizedError,
};

pub const JSON_STATUS_INVOCATION: &[&str] = &["status", "--json"];
pub const STATUS_INVOCATION: &[&str] = &["status"];
pub const SELECT_INVOCATION_PREFIX: &[&str] = &["profile", "select"];
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(
        &self,
        executable: &Path,
        arguments: &[OsString],
        timeout: Duration,
    ) -> std::result::Result<CommandOutput, CommandRunError>;
}

#[derive(Debug)]
pub enum CommandRunError {
    Io(std::io::Error),
    TimedOut,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TokioCommandRunner;

#[async_trait]
impl CommandRunner for TokioCommandRunner {
    async fn run(
        &self,
        executable: &Path,
        arguments: &[OsString],
        timeout: Duration,
    ) -> std::result::Result<CommandOutput, CommandRunError> {
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = command.spawn().map_err(CommandRunError::Io)?;
        let output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| CommandRunError::TimedOut)?
            .map_err(CommandRunError::Io)?;
        Ok(CommandOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

#[async_trait]
pub trait NetbirdApi: Send + Sync {
    async fn active_profile(&self) -> std::result::Result<String, SanitizedError>;
    async fn select_profile(&self, target: &str) -> std::result::Result<(), SanitizedError>;

    fn set_executable(&mut self, _executable: &Path) -> std::result::Result<(), SanitizedError> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct NetbirdClient<R> {
    executable: PathBuf,
    runner: R,
    timeout: Duration,
}

impl<R> NetbirdClient<R> {
    pub fn new(executable: PathBuf, runner: R, timeout: Duration) -> Result<Self> {
        if !executable.is_absolute() {
            return Err(HawkError::Validation(
                "NetBird executable path must be absolute".to_owned(),
            ));
        }
        Ok(Self {
            executable,
            runner,
            timeout,
        })
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

impl NetbirdClient<TokioCommandRunner> {
    pub fn discover() -> Result<Self> {
        let found = which::which("netbird").map_err(|_| HawkError::NetbirdNotFound)?;
        let absolute = if found.is_absolute() {
            found
        } else {
            std::env::current_dir()
                .map_err(|source| HawkError::io("read current directory", ".", source))?
                .join(found)
        };
        Self::new(absolute, TokioCommandRunner, DEFAULT_COMMAND_TIMEOUT)
    }
}

#[async_trait]
impl<R: CommandRunner> NetbirdApi for NetbirdClient<R> {
    async fn active_profile(&self) -> std::result::Result<String, SanitizedError> {
        let json_arguments = JSON_STATUS_INVOCATION
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        match self
            .runner
            .run(&self.executable, &json_arguments, self.timeout)
            .await
        {
            Ok(output) if output.status.success() => {
                if let Ok(profile) = parse_json_active_profile(&output.stdout) {
                    return Ok(profile);
                }
            }
            Ok(_) => {}
            Err(CommandRunError::TimedOut) => {
                return Err(SanitizedError::timed_out("status"));
            }
            Err(CommandRunError::Io(_)) => {
                return Err(SanitizedError::command_failed("status", None));
            }
        }

        // Older NetBird versions, or structured payloads without profileName,
        // fall back to the documented human-readable Profile field contract.
        let arguments = STATUS_INVOCATION
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        let output = self
            .runner
            .run(&self.executable, &arguments, self.timeout)
            .await
            .map_err(|error| sanitize_runner_error("status", error))?;
        if !output.status.success() {
            return Err(SanitizedError::command_failed(
                "status",
                output.status.code(),
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_active_profile(&stdout).map_err(SanitizedError::malformed_status)
    }

    async fn select_profile(&self, target: &str) -> std::result::Result<(), SanitizedError> {
        let arguments = [
            OsString::from(SELECT_INVOCATION_PREFIX[0]),
            OsString::from(SELECT_INVOCATION_PREFIX[1]),
            OsString::from(target),
        ];
        let output = self
            .runner
            .run(&self.executable, &arguments, self.timeout)
            .await
            .map_err(|error| sanitize_runner_error("profile selection", error))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(SanitizedError::command_failed(
                "profile selection",
                output.status.code(),
            ))
        }
    }

    fn set_executable(&mut self, executable: &Path) -> std::result::Result<(), SanitizedError> {
        if !executable.is_absolute() {
            return Err(SanitizedError {
                category: crate::model::ErrorCategory::Configuration,
                message: "saved NetBird executable path is not absolute".to_owned(),
                exit_code: None,
                timed_out: false,
            });
        }
        self.executable = executable.to_path_buf();
        Ok(())
    }
}

pub fn parse_json_active_profile(output: &[u8]) -> std::result::Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_slice(output).map_err(|_| "structured status was invalid JSON")?;
    let profile = value
        .get("profileName")
        .and_then(serde_json::Value::as_str)
        .ok_or("structured status omitted profileName")?
        .trim();
    if profile.is_empty() {
        Err("structured status contained an empty profileName".to_owned())
    } else {
        Ok(profile.to_owned())
    }
}

pub fn parse_active_profile(output: &str) -> std::result::Result<String, String> {
    let mut profile = None;
    for line in output.lines() {
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        if field.trim() != "Profile" {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            return Err("the Profile field was empty".to_owned());
        }
        if profile.replace(value.to_owned()).is_some() {
            return Err("more than one Profile field was returned".to_owned());
        }
    }
    profile.ok_or_else(|| "the Profile field was missing".to_owned())
}

fn sanitize_runner_error(operation: &str, error: CommandRunError) -> SanitizedError {
    match error {
        CommandRunError::TimedOut => SanitizedError::timed_out(operation),
        CommandRunError::Io(_) => SanitizedError::command_failed(operation, None),
    }
}

pub fn operational_error(error: &SanitizedError) -> HawkError {
    if error.timed_out {
        HawkError::NetbirdTimeout {
            operation: "command",
        }
    } else if error.category == crate::model::ErrorCategory::MalformedStatus {
        HawkError::NetbirdStatus(error.message.clone())
    } else {
        HawkError::NetbirdCommand {
            operation: "command",
            exit_code: error.exit_code,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use super::*;

    #[derive(Debug)]
    struct FakeRunner {
        outputs: Mutex<VecDeque<std::result::Result<CommandOutput, CommandRunError>>>,
        calls: Mutex<Vec<Vec<OsString>>>,
    }

    #[async_trait]
    impl CommandRunner for FakeRunner {
        async fn run(
            &self,
            _executable: &Path,
            arguments: &[OsString],
            _timeout: Duration,
        ) -> std::result::Result<CommandOutput, CommandRunError> {
            self.calls.lock().unwrap().push(arguments.to_vec());
            self.outputs.lock().unwrap().pop_front().unwrap()
        }
    }

    #[cfg(unix)]
    fn status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn status(code: i32) -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(code as u32)
    }

    fn client(
        outputs: Vec<std::result::Result<CommandOutput, CommandRunError>>,
    ) -> NetbirdClient<FakeRunner> {
        NetbirdClient::new(
            std::env::current_exe().unwrap(),
            FakeRunner {
                outputs: Mutex::new(outputs.into()),
                calls: Mutex::new(Vec::new()),
            },
            Duration::from_millis(1),
        )
        .unwrap()
    }

    #[test]
    fn extracts_one_profile_independent_of_field_order() {
        assert_eq!(
            parse_active_profile("Management: Connected\nProfile: default\nOS: linux\n"),
            Ok("default".to_owned())
        );
        assert_eq!(
            parse_active_profile("  Profile : work profile  \nOther: x"),
            Ok("work profile".to_owned())
        );
    }

    #[test]
    fn extracts_the_documented_structured_profile_name() {
        assert_eq!(
            parse_json_active_profile(br#"{"peers":[],"profileName":"work"}"#),
            Ok("work".to_owned())
        );
        assert!(parse_json_active_profile(br#"{"profileName":""}"#).is_err());
        assert!(parse_json_active_profile(br#"{"other":"work"}"#).is_err());
    }

    #[test]
    fn rejects_missing_empty_and_duplicate_profile_fields() {
        assert!(parse_active_profile("Status: Connected").is_err());
        assert!(parse_active_profile("Profile:   ").is_err());
        assert!(parse_active_profile("Profile: one\nProfile: two").is_err());
    }

    #[tokio::test]
    async fn nonzero_timeout_and_unexpected_output_are_sanitized() {
        let secret = "token=very-secret Authorization: Bearer abc cookie=session";
        let nonzero = client(vec![
            Ok(CommandOutput {
                status: status(7),
                stdout: secret.as_bytes().to_vec(),
                stderr: secret.as_bytes().to_vec(),
            }),
            Ok(CommandOutput {
                status: status(7),
                stdout: secret.as_bytes().to_vec(),
                stderr: secret.as_bytes().to_vec(),
            }),
        ]);
        let timed_out = client(vec![Err(CommandRunError::TimedOut)]);
        let unexpected = client(vec![
            Ok(CommandOutput {
                status: status(0),
                stdout: secret.as_bytes().to_vec(),
                stderr: Vec::new(),
            }),
            Ok(CommandOutput {
                status: status(0),
                stdout: secret.as_bytes().to_vec(),
                stderr: Vec::new(),
            }),
        ]);

        for client in [&nonzero, &timed_out, &unexpected] {
            let serialized =
                serde_json::to_string(&client.active_profile().await.unwrap_err()).unwrap();
            assert!(!serialized.contains("very-secret"));
            assert!(!serialized.contains("Bearer"));
            assert!(!serialized.contains("session"));
        }
    }

    #[tokio::test]
    async fn invokes_status_and_select_without_a_shell() {
        let client = client(vec![
            Ok(CommandOutput {
                status: status(0),
                stdout: br#"{"profileName":"alpha"}"#.to_vec(),
                stderr: Vec::new(),
            }),
            Ok(CommandOutput {
                status: status(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            }),
        ]);
        assert_eq!(client.active_profile().await.unwrap(), "alpha");
        client.select_profile("beta; echo unsafe").await.unwrap();
        let calls = client.runner.calls.lock().unwrap();
        assert_eq!(
            calls[0],
            [OsString::from("status"), OsString::from("--json")]
        );
        assert_eq!(
            calls[1],
            [
                OsString::from("profile"),
                OsString::from("select"),
                OsString::from("beta; echo unsafe")
            ]
        );
    }

    #[tokio::test]
    async fn falls_back_to_the_profile_text_field() {
        let client = client(vec![
            Ok(CommandOutput {
                status: status(2),
                stdout: Vec::new(),
                stderr: b"unknown flag --json".to_vec(),
            }),
            Ok(CommandOutput {
                status: status(0),
                stdout: b"OS: linux\nProfile: alpha\n".to_vec(),
                stderr: Vec::new(),
            }),
        ]);
        assert_eq!(client.active_profile().await.unwrap(), "alpha");
    }
}
