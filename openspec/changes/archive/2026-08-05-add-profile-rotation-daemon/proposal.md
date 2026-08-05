## Why

NetBird profiles need to be rotated at a predictable local time so that SSO session renewal does not interrupt the workday. The application currently has no command-line contract, background lifecycle, or daily profile-rotation behavior.

## What Changes

- Add a `start` command accepting one or more ordered `--profile`/`-p` values and a required local `--time`/`-t` value in 24-hour `H:MM` or `HH:MM` format.
- Start a per-user background daemon, or atomically replace its configuration and reschedule it when it is already running.
- Read the active profile from `netbird status` at startup and before each scheduled rotation.
- At the configured local wall-clock time, select the profile after the currently active profile in the supplied order, wrapping to the first profile, and repeat daily.
- Add `stop` and `status` commands for daemon lifecycle control and expose command help through `help` and `--help`.
- Define recovery behavior for restarts, system sleep, local clock or timezone changes, malformed NetBird output, command failures, and stale daemon state.
- Use established Rust ecosystem libraries for CLI parsing, async scheduling, serialization, logging, filesystem locations, and process coordination instead of custom infrastructure where mature libraries exist.

## Capabilities

### New Capabilities

- `daemon-cli-control`: Command-line parsing, validation, help, and cross-platform start/reconfigure/stop/status control of the per-user daemon.
- `profile-rotation-scheduling`: Active-profile discovery and reliable daily rotation through an ordered profile list using the computer's local time.

### Modified Capabilities

None.

## Impact

- Replaces the placeholder Rust entry point with a structured CLI and daemon runtime.
- Adds persisted non-secret daemon configuration and runtime metadata in OS-appropriate per-user locations.
- Executes the installed `netbird` CLI (`status` and `profile select`) and depends on the stability of its documented command behavior and the `Profile:` status field.
- Adds mature crates for CLI parsing and runtime concerns, with platform-specific process integration isolated behind shared abstractions.
- Introduces observable background-process, logging, scheduling, and error-reporting behavior on macOS, Windows, and Linux.
