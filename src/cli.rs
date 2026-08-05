use std::{io::Write, path::PathBuf};

use clap::{Args, Parser, Subcommand};

use crate::{
    controller::LifecycleController,
    daemon,
    error::Result,
    model::{LocalTime, validate_profiles},
    netbird::NetbirdClient,
    platform::DetachedProcessLauncher,
    state::StateStore,
};

#[derive(Debug, Parser)]
#[command(
    name = "netbird-hawk",
    version,
    about = "Rotate NetBird profiles at a predictable local time",
    subcommand_required = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the per-user daemon, or atomically replace its schedule.
    Start(StartArgs),
    /// Gracefully stop the current user's daemon.
    Stop,
    /// Show the current user's daemon state.
    Status,
    /// Internal detached worker entry point.
    #[command(name = "__worker", hide = true)]
    Worker,
}

#[derive(Debug, Args)]
pub struct StartArgs {
    /// Profile handle in circular rotation order (repeatable).
    #[arg(short = 'p', long = "profile", required = true, action = clap::ArgAction::Append)]
    pub profiles: Vec<String>,

    /// Daily local wall-clock time in H:MM or HH:MM format.
    #[arg(short = 't', long = "time", required = true)]
    pub local_time: LocalTime,
}

impl StartArgs {
    pub fn validate(&self) -> Result<()> {
        validate_profiles(&self.profiles)
    }
}

pub async fn execute(cli: Cli) -> Result<String> {
    let store = StateStore::from_system()?;
    match cli.command {
        Command::Start(arguments) => {
            arguments.validate()?;
            let netbird = NetbirdClient::discover()?;
            let executable = netbird.executable().to_path_buf();
            let current_executable = std::env::current_exe().map_err(|source| {
                crate::error::HawkError::io(
                    "resolve current executable",
                    PathBuf::from("."),
                    source,
                )
            })?;
            LifecycleController::new(store, DetachedProcessLauncher)
                .start(
                    &netbird,
                    executable,
                    &current_executable,
                    arguments.profiles,
                    arguments.local_time,
                )
                .await
        }
        Command::Stop => {
            LifecycleController::new(store, DetachedProcessLauncher)
                .stop()
                .await
        }
        Command::Status => LifecycleController::new(store, DetachedProcessLauncher).status(),
        Command::Worker => {
            daemon::run_worker(store).await?;
            Ok(String::new())
        }
    }
}

pub async fn entrypoint() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let exit_code = error.exit_code();
            let _ = error.print();
            return exit_code;
        }
    };
    match execute(cli).await {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            0
        }
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "error: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn mixed_short_and_long_start_arguments_preserve_order() {
        let cli = Cli::try_parse_from([
            "netbird-hawk",
            "start",
            "--profile",
            "default",
            "-p",
            "default2",
            "--time",
            "8:24",
        ])
        .unwrap();
        let Command::Start(arguments) = cli.command else {
            panic!("expected start command");
        };
        arguments.validate().unwrap();
        assert_eq!(arguments.profiles, ["default", "default2"]);
        assert_eq!(arguments.local_time.to_string(), "08:24");
    }

    #[test]
    fn generated_help_exposes_public_commands_but_not_worker() {
        let help = Cli::command().render_long_help().to_string();
        for command in ["start", "stop", "status", "help"] {
            assert!(help.contains(command), "root help omitted {command}");
        }
        assert!(!help.contains("__worker"));
    }

    #[test]
    fn missing_and_invalid_values_are_parse_errors() {
        assert!(Cli::try_parse_from(["netbird-hawk", "start", "--time", "08:00"]).is_err());
        assert!(
            Cli::try_parse_from([
                "netbird-hawk",
                "start",
                "--profile",
                "alpha",
                "--time",
                "24:00"
            ])
            .is_err()
        );
    }

    #[test]
    fn duplicate_and_empty_profiles_fail_domain_validation() {
        for profiles in [vec!["alpha", "alpha"], vec![""]] {
            let arguments = StartArgs {
                profiles: profiles.into_iter().map(str::to_owned).collect(),
                local_time: "08:00".parse().unwrap(),
            };
            assert!(arguments.validate().is_err());
        }
    }
}
