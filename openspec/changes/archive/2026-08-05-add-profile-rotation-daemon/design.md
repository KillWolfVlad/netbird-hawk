## Context

See `proposal.md` for motivation and the two delta specs for observable behavior. The repository currently contains only a placeholder Rust binary and no dependencies. The daemon must be a per-user, unattended process on macOS, Windows, and Linux; it must invoke an independently installed NetBird CLI while remaining testable without NetBird or real wall-clock waits.

The active-profile integration is intentionally limited to the user-provided NetBird contract: `netbird status` contains a `Profile:` field and switching uses `netbird profile select <name>`. Profile enumeration and undocumented output formats are not assumed.

## Goals / Non-Goals

**Goals:**

- Keep CLI, lifecycle, scheduler, persistence, clock, and NetBird process execution behind narrow interfaces so the core can be tested deterministically.
- Guarantee one daemon per OS user and serialize concurrent lifecycle changes.
- Make configuration replacement and runtime snapshots atomic and crash-recoverable.
- Preserve local-calendar semantics through sleep, restart, timezone changes, and daylight-saving transitions.
- Prefer mature, widely adopted Rust libraries and standard runtime facilities over custom parsers, schedulers, or shell command construction.

**Non-Goals:**

- Installing an OS service or automatically starting at login/boot; `start` launches a detached per-user process that runs until `stop`, failure, or OS shutdown.
- Managing NetBird installation, login, SSO browser interaction, credentials, or profile creation.
- Discovering or validating every available profile through undocumented NetBird commands.
- Providing a remote control API, graphical UI, or multi-user system daemon.

## Decisions

### 1. Use a typed `clap` command tree

Use `clap` derive for public `start`, `stop`, `status`, and `help` behavior and one hidden internal worker entry point. `start` uses a repeatable `Vec<String>` argument with both `--profile` and `-p`, plus a typed local-time parser for `--time`/`-t`. Domain validation then rejects empty/duplicate names and normalizes the time to `NaiveTime`.

`clap` is a mature, high-adoption Rust CLI project with native subcommands, aliases, repeated values, generated help, typed value parsers, and consistent error exits. A hand-written parser was rejected because it would duplicate established validation and help behavior. `argh` and `lexopt` were considered, but `clap` better matches the requested multi-command UX and ecosystem criterion.

### 2. Separate adapters from a deterministic core

Organize the binary into modules with traits at integration boundaries:

- CLI/controller: validates commands and coordinates lifecycle acknowledgements.
- Daemon runtime: owns cancellation, configuration generations, retries, and status publication.
- Scheduler: pure calculations over an injected wall clock plus an async wake primitive.
- NetBird client: injected command runner and a pure status parser.
- State store: atomic configuration, control intent, execution journal, and runtime snapshots.
- Platform process adapter: detached worker launch and OS-specific process options.

Use `tokio` for the daemon event loop, cancellation, process timeouts, and asynchronous `Command` execution. Use `chrono` for `Local`, `NaiveTime`, local dates, and explicit ambiguous/nonexistent local-time handling. Both are established ecosystem libraries; the design avoids a cron-expression layer because the input is a single daily local wall time and explicit date identity is needed for catch-up and de-duplication.

### 3. Run as a detached child, not an installed system service

`start` launches the same executable with a hidden worker mode, null standard streams, and paths/generation passed through internal arguments. On Unix, the platform adapter creates a new session using the well-established `nix` bindings; on Windows it uses official `windows-sys` process flags for a detached process and new process group. The worker writes structured rolling logs instead of inheriting the terminal.

Installing launchd, systemd, or Windows services was rejected for this change: it changes machine configuration, often needs elevated privileges, and does not provide one consistent per-user contract. A foreground-only command was rejected because it would not satisfy background daemon behavior. OS startup integration can be a later capability.

### 4. Coordinate through atomic per-user files and locks

Use `directories` to resolve config, state, and log roots. Use `fs2` advisory locks for two roles: a short-lived controller lock serializes `start`/`stop`, while a daemon lock is held for the worker lifetime and is the source of truth for liveness. A PID is diagnostic only and is never sufficient justification to signal or kill a process.

Persist versioned JSON with `serde`/`serde_json` and replace files atomically by writing, syncing, and persisting a `tempfile` in the same directory. Important records are:

- desired configuration: schema version, generation UUID, ordered profiles, local time, resolved NetBird executable path, activation timestamp, and desired running state;
- execution journal: generation, local date, original profile, intended target, attempt count, and outcome;
- runtime snapshot: lifecycle state, applied generation, PID, discovered/next profiles, next local occurrence, last result, and sanitized error.

The worker watches for changes with a low-frequency async reconciliation tick. `start` and `stop` wait until the runtime snapshot acknowledges their generation (or until the daemon lock is released for stop). This simple persisted control plane survives missed filesystem notifications and enables restart reconciliation without exposing a listening socket. OS-native IPC was considered, but its endpoint and authentication differences add complexity without improving the small, local command surface.

On Unix, directories/files are created with user-only permissions (`0700`/`0600` where applicable). Windows relies on the user's profile-directory ACLs and avoids broadening them. Only allowlisted fields are persisted; raw stdout/stderr from NetBird is kept in memory only long enough to parse and is summarized on failure.

### 5. Preflight before publishing a new configuration

The controller resolves `netbird` to an absolute executable path using the established `which` crate, executes `netbird status` without a shell, applies a bounded timeout, and parses exactly one non-empty `Profile:` line. A failed preflight prevents initial start and prevents a running daemon from replacing its known-good generation.

Once spawned or reconfigured, the worker performs its own startup discovery before acknowledging the generation. This closes the gap between controller and worker environments and satisfies the requirement that ready state includes an active-profile read. Profile existence is not prevalidated because the provided contract does not define a listing command; selection errors remain visible and recoverable.

### 6. Treat configuration as immutable generations

Every accepted `start` creates a UUID generation. The daemon loads the whole record, validates it again, cancels the previous generation's timer and retry token, computes the first occurrence strictly after activation, publishes the applied generation, and only then allows `start` to return success. Old async work carries its generation and checks it before each external side effect, preventing a stale retry after reconfiguration.

If acknowledgement times out, the controller compares the published generation and reports whether the old one remains active. Partial in-memory application is prevented by constructing and validating a replacement runtime before swapping it in.

### 7. Recompute local occurrences instead of trusting one long monotonic sleep

Represent occurrence identity as `(generation, local_date)`. On each reconciliation wake, after system resume, and at a bounded guard interval, read the OS local clock again and recompute the canonical instant for the configured local date:

- normal local time: use the single instant;
- ambiguous fall-back time: use the earliest instant;
- nonexistent spring-forward time: use the first valid instant after the gap.

The async wait never extends beyond a short guard interval before rechecking wall time, so manual clock/timezone changes are observed without OS-specific notification APIs. New generations schedule their first occurrence strictly in the future. Existing generations with an unhandled due occurrence catch up only the most recent applicable local date; the journal prevents duplicate execution and historical bursts.

Long one-shot sleeps alone were rejected because monotonic timers do not reliably express changed wall-clock intent. A cron library was rejected because most cron engines hide DST, catch-up, and occurrence identity semantics that this product must make explicit.

### 8. Journal intent before selecting and reconcile idempotently

At a due occurrence, read the active profile, calculate the circular successor, and atomically persist `in_progress` with original and target profiles before invoking `netbird profile select <target>`. Execute the binary and arguments directly with `tokio::process::Command`; never interpolate a shell string. Apply a timeout and a small fixed bounded retry schedule with delays kept as constants behind a policy interface for deterministic tests.

Before a retry or after restart, read status again:

- active equals target: record success without reselecting;
- active equals original: retry the same target if the policy permits;
- active differs from both: record `superseded` and preserve the user's manual selection.

Mark the occurrence terminal before scheduling the next date. Persisting intent before the side effect narrows crash ambiguity; reconciling the active profile makes selection effectively idempotent and avoids advancing twice after a crash.

### 9. Publish sanitized structured diagnostics

Use `tracing`, `tracing-subscriber`, and `tracing-appender` for structured, bounded file logging. Public commands print concise human-readable summaries. Errors use typed context (`thiserror` for domain errors and `anyhow` at process boundaries) and store only error category, exit status, timeout indicator, and a bounded sanitized message. Tests assert that raw command output and credential-like values are not persisted.

## Risks / Trade-offs

- [Detached child is not automatically restarted after OS reboot or a fatal crash] → Document this boundary, make `status` truthful, preserve configuration/journal for safe manual restart, and leave login/boot integration to a separate capability.
- [NetBird changes the human-readable `status` format] → Keep parsing isolated, accept field reordering, fail closed on missing/duplicate profile fields, and cover captured fixtures; investigate a structured NetBird output mode before implementation if one is documented.
- [Filesystem locks vary on unusual/network filesystems] → Store state only in OS-local per-user directories, use the lock rather than PID as liveness truth, and test concurrent starts on all three target OS families.
- [Wall-clock polling adds up to one guard interval of scheduling latency] → Keep the interval short and configurable in tests; correctness across clock changes is prioritized over sub-second precision for a daily task.
- [A manual change concurrent with the first selection command cannot always be distinguished] → Re-read before retries, preserve any third profile, and guarantee bounded rather than looping writes; exact simultaneous ordering follows whichever NetBird command completes last.
- [A configuration write can succeed while acknowledgement later times out] → Report the observed applied generation explicitly and keep generation checks so users can safely retry `start`.
- [Profile names appear in state and logs] → Treat state as user-private, apply restrictive permissions, and never persist broader NetBird output or authentication material.

## Migration Plan

1. Add dependencies with pinned compatible version ranges and run license/advisory checks.
2. Replace the placeholder binary with the modular CLI and internal worker while retaining package name `netbird-hawk`.
3. Introduce a versioned state schema at version 1; because no prior daemon state exists, first start creates the directories and files.
4. Validate unit, integration, concurrency, restart, sleep/clock simulation, and cross-platform build/test coverage before release.
5. Roll back by stopping the daemon and restoring the prior binary. Versioned state is inert when the old placeholder runs and may be retained for a later reinstall; no NetBird credentials or system-service registrations need removal.

## Open Questions

- During implementation, verify whether the supported NetBird CLI version offers a documented machine-readable status mode. If it does, the NetBird adapter may prefer it while preserving the specified observable behavior and text fallback.
