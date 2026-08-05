## 1. Dependency and Module Setup

- [x] 1.1 Verify the selected crates against their official repositories, maintenance status, licenses, security advisories, and relevant Rust awesome-list entries, then add minimal feature-gated dependencies for `clap`, `tokio`, `chrono`, persistence, locking, paths, diagnostics, executable discovery, UUID generations, Unix detachment, and Windows process bindings.
- [x] 1.2 Replace the placeholder entry point with module boundaries for CLI/controller, daemon runtime, scheduler, NetBird adapter, state store, and platform process launching while keeping integration boundaries injectable.
- [x] 1.3 Define typed domain errors, command exit handling, and test-only clock/command/process abstractions so normal failures never require panics.

## 2. CLI Contract and Validation

- [x] 2.1 Implement the `clap` command tree for `start`, `stop`, `status`, root `help`/`--help`, subcommand help, and a hidden internal worker command.
- [x] 2.2 Implement typed parsing and domain validation for repeatable ordered `--profile`/`-p` values and `H:MM`/`HH:MM` local `--time`/`-t`, including missing, empty, duplicate, and out-of-range errors.
- [x] 2.3 Add CLI tests covering the documented mixed short/long example, help surfaces, validation errors, stdout/stderr separation, and successful versus unsuccessful exit codes.

## 3. Per-User State and Coordination

- [x] 3.1 Resolve OS-appropriate per-user config, state, and log directories and enforce restrictive Unix permissions without broadening Windows profile ACLs.
- [x] 3.2 Define versioned serialized records for desired configuration, immutable generation identity, execution journal, control intent, and sanitized runtime snapshot.
- [x] 3.3 Implement same-directory atomic write/sync/replace and robust read/validation paths for all persisted records, including interrupted-write and unsupported-version tests.
- [x] 3.4 Implement controller-operation and daemon-lifetime locks, make the lifetime lock the liveness source of truth, and test stale PID/metadata and concurrent ownership cases.

## 4. NetBird Command Adapter

- [x] 4.1 Check supported NetBird documentation for a machine-readable status option and record the chosen documented invocation while retaining the specified `Profile:` text parser fallback.
- [x] 4.2 Implement absolute NetBird executable discovery plus shell-free, timeout-bounded async execution for `netbird status` and `netbird profile select <target>`.
- [x] 4.3 Implement strict extraction of one non-empty `Profile:` field independent of field order, with fixture tests for valid, missing, empty, duplicate, non-zero, timed-out, and unexpected outputs.
- [x] 4.4 Implement allowlisted error sanitization and tests proving raw status output, credentials, tokens, cookies, and authorization codes are not written to state or normal logs.

## 5. Local-Time Scheduler and Rotation Core

- [x] 5.1 Implement and unit-test circular successor selection for middle, wraparound, out-of-list, manually changed, and one-profile cases.
- [x] 5.2 Implement pure next/due occurrence calculation keyed by generation and local date, including activation after today's time, timezone/clock changes, DST gaps, DST folds, and short wall-clock guard rechecks.
- [x] 5.3 Implement catch-up selection that handles only the latest due date after sleep or downtime and suppresses already handled occurrences and historical bursts.
- [x] 5.4 Implement write-ahead execution intent and restart reconciliation for target-already-active, original-still-active, and third-profile-manual-override outcomes.
- [x] 5.5 Implement the bounded retry policy and cancellation conditions for success, manual supersession, retry exhaustion, next occurrence, configuration replacement, and stop.

## 6. Daemon Runtime and Lifecycle Commands

- [x] 6.1 Implement the Tokio daemon event loop that acquires lifetime ownership, performs startup discovery, publishes ready/degraded state, reconciles control generations, runs due occurrences, and atomically updates status.
- [x] 6.2 Implement immutable configuration-generation replacement so the old schedule and retries are cancelled before the new generation is acknowledged and stale tasks cannot execute external side effects.
- [x] 6.3 Implement detached worker launch through isolated Unix (`nix`) and Windows (`windows-sys`) adapters with null terminal streams, bounded ready acknowledgement, and no installed system service.
- [x] 6.4 Implement `start` preflight, serialized initial launch, existing-daemon reconfiguration, acknowledgement timeout diagnostics, and two-concurrent-start behavior.
- [x] 6.5 Implement idempotent graceful `stop`, bounded shutdown acknowledgement, cancellation of pending work, and safe timeout behavior that never kills from PID data alone.
- [x] 6.6 Implement `status` reporting for running, starting, degraded, and stopped states with ordered profiles, local time, active/next profile, next occurrence, and sanitized last result.
- [x] 6.7 Configure bounded structured file logging and verify that the detached process remains independent of the invoking terminal on macOS, Linux, and Windows.

## 7. End-to-End Reliability Verification

- [x] 7.1 Build a fake NetBird executable test harness and cover start readiness, daily selection, manual profile changes, selection failures/timeouts, retry recovery, and stop without requiring a real NetBird session.
- [x] 7.2 Add restart/crash-window tests for write-ahead intent, same-target reconciliation, stale state, sleep catch-up, multi-day downtime, and no duplicate rotation for one generation/date.
- [x] 7.3 Add lifecycle integration tests for atomic reconfiguration, invalid-update preservation, concurrent commands, acknowledgement timeouts, and cancellation of stale retries.
- [x] 7.4 Add or update CI to build and test supported Rust targets on macOS, Windows, and Linux, including platform-specific detached-launch compilation and targeted runtime tests.
- [x] 7.5 Run formatting, linting with warnings denied, the full test suite, dependency license/advisory checks, and release-mode builds for all available target environments; resolve every failure.

## 8. User Documentation

- [x] 8.1 Update the README with installation prerequisites, the exact `start` example, profile-order semantics, local-time/DST and sleep behavior, and `stop`/`status`/help usage.
- [x] 8.2 Document per-OS state/log locations, privacy guarantees, troubleshooting for missing NetBird or malformed status, safe recovery from a stuck daemon, and the explicit lack of boot/login autostart.
