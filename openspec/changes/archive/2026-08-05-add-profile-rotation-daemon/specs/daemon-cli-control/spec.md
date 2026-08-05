## Purpose

Defines the command-line contract and reliable per-user daemon lifecycle used to configure, inspect, reconfigure, and stop profile rotation on every supported operating system.

## ADDED Requirements

### Requirement: The CLI exposes lifecycle commands and generated help
The application SHALL expose `start`, `stop`, `status`, and `help` commands. It SHALL use an established command-line parsing library to generate usage and validation errors, and both `netbird-hawk help` and `netbird-hawk --help` SHALL display root help that documents every public command.

#### Scenario: Root help is requested with the flag
- **WHEN** the user runs `netbird-hawk --help`
- **THEN** the application exits successfully and prints usage for `start`, `stop`, `status`, and `help`

#### Scenario: Root help is requested with the command
- **WHEN** the user runs `netbird-hawk help`
- **THEN** the application exits successfully and prints the same root command documentation

#### Scenario: Subcommand help is requested
- **WHEN** the user requests help for a public subcommand
- **THEN** the application exits successfully and documents that subcommand's arguments and aliases

### Requirement: Start configuration is validated before it is applied
The `start` command SHALL require at least one `--profile` or `-p` value and exactly one `--time` or `-t` value. It SHALL preserve profile order, reject empty or duplicate profile names, and accept local 24-hour times in `H:MM` or `HH:MM` form only when the hour is 0 through 23 and the minute is 0 through 59.

#### Scenario: Long and short profile flags are combined
- **WHEN** the user runs `netbird-hawk start --profile default -p default2 --time 8:24`
- **THEN** the accepted ordered profile list is `default`, `default2` and the accepted local time is `08:24`

#### Scenario: A required argument is missing
- **WHEN** the user omits all profiles or omits the time
- **THEN** the application exits unsuccessfully, prints a concise validation error and relevant usage, and does not change a running daemon's configuration

#### Scenario: The time is invalid
- **WHEN** the user supplies a value outside the accepted 24-hour format or range
- **THEN** the application exits unsuccessfully and does not start or reconfigure the daemon

#### Scenario: A profile is duplicated
- **WHEN** the same profile name is supplied more than once
- **THEN** the application exits unsuccessfully and explains that profiles must be unique

### Requirement: Start creates one ready background daemon
After input validation and a successful NetBird preflight, `start` SHALL ensure that exactly one daemon runs for the current OS user. When no daemon is running, it SHALL launch the daemon independently of the invoking terminal and SHALL not report success until the daemon has acquired single-instance ownership, loaded the configuration, read the active profile, and published ready state.

#### Scenario: A daemon is started
- **WHEN** valid configuration is supplied and no daemon is running
- **THEN** `start` launches a background daemon, waits for its ready acknowledgement, prints a success summary, and exits successfully

#### Scenario: Daemon initialization fails
- **WHEN** the child cannot acquire ownership, load the configuration, execute the startup status check, or publish ready state within a bounded startup timeout
- **THEN** `start` exits unsuccessfully with a diagnostic and does not claim that the daemon is running

#### Scenario: Two start commands race
- **WHEN** two valid `start` invocations overlap for the same OS user
- **THEN** lifecycle operations are serialized and no more than one daemon remains running

### Requirement: Start atomically reconfigures an existing daemon
When the current user's daemon is already running, `start` SHALL replace its active profile list and local time as one configuration generation, cancel the previous schedule, and acknowledge only after the daemon has applied the new generation. It SHALL NOT launch a second daemon.

#### Scenario: A running daemon receives new parameters
- **WHEN** the user runs `start` with valid new parameters while the daemon is running
- **THEN** the daemon atomically applies the complete new configuration, recreates its schedule, publishes the new generation, and continues under the same single-instance ownership

#### Scenario: Reconfiguration cannot be applied
- **WHEN** the daemon rejects or cannot acknowledge the new generation
- **THEN** `start` exits unsuccessfully, reports whether the daemon is still using its previous generation, and never reports partial application

### Requirement: Stop terminates the current user's daemon
The `stop` command SHALL request graceful termination, wait for bounded acknowledgement, and report the result. Stopping SHALL cancel pending timers and retries without executing another profile change.

#### Scenario: A running daemon is stopped
- **WHEN** the user runs `netbird-hawk stop` while the daemon is running
- **THEN** the daemon records stopped state, releases single-instance ownership, terminates, and the command exits successfully

#### Scenario: Stop is repeated
- **WHEN** the user runs `stop` while no daemon is running
- **THEN** the command reports that the daemon is already stopped and exits successfully without treating stale metadata as a live process

#### Scenario: Graceful stop times out
- **WHEN** a daemon appears to own the instance but does not acknowledge the stop request within the configured timeout
- **THEN** the command exits unsuccessfully with recovery guidance and SHALL NOT kill an unrelated process based only on a stale process identifier

### Requirement: Status reports observable daemon state
The `status` command SHALL report whether the current user's daemon is `running`, `starting`, `degraded`, or `stopped`. For a live daemon it SHALL also report the applied profiles in order, configured local time, active profile when known, next target when known, next scheduled local occurrence, and the last rotation result or error without exposing sensitive NetBird data.

#### Scenario: Live status is requested
- **WHEN** the user runs `netbird-hawk status` while the daemon owns the instance
- **THEN** the command prints the daemon state and its latest atomically published runtime summary

#### Scenario: Stopped status is requested
- **WHEN** the user runs `status` while no daemon owns the instance
- **THEN** the command prints `stopped`, ignores stale runtime metadata as evidence of liveness, and exits successfully

#### Scenario: Runtime metadata is temporarily unreadable
- **WHEN** the daemon is live but its status snapshot cannot be parsed
- **THEN** the command reports `degraded`, explains that runtime details are unavailable, and does not crash

### Requirement: Lifecycle state is per-user, durable, and non-sensitive
The application SHALL keep configuration, control state, locks, logs, and runtime summaries in OS-appropriate per-user locations with restrictive access. It SHALL persist only the data required to resume scheduling and reconcile an interrupted rotation, and SHALL never log or persist credentials, session tokens, cookies, authorization codes, or complete unfiltered NetBird command output.

#### Scenario: A daemon is restarted with existing state
- **WHEN** a daemon starts after an interruption with a valid saved configuration and execution record
- **THEN** it reconstructs the schedule and reconciles any interrupted rotation without requiring secrets

#### Scenario: State from another OS user exists
- **WHEN** a user starts or controls netbird-hawk
- **THEN** only that user's daemon and state locations are read or modified

#### Scenario: An unexpected NetBird response contains sensitive text
- **WHEN** an external command returns output not explicitly selected for an allowed status field
- **THEN** the raw output is not copied into persistent state or normal logs

### Requirement: Public command failures are scriptable
Public commands SHALL return success only when their requested operation or informational output completes as documented. Validation, NetBird preflight, lifecycle timeout, state I/O, and daemon-control failures SHALL produce a non-zero exit code and a concise error on standard error; normal results and help SHALL be written to standard output.

#### Scenario: A command fails
- **WHEN** a public command encounters an operational error
- **THEN** it emits a human-readable diagnostic to standard error and returns a non-zero exit code without a panic backtrace in normal operation

