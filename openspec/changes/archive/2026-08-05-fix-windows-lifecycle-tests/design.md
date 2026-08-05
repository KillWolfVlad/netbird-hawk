## Context

See `proposal.md` for motivation and `specs/daemon-cli-control/spec.md` for the observable contract. `StateStore::try_lifetime_lock` currently classifies contention only by `std::io::ErrorKind::WouldBlock`; `fs2` instead exposes Win32 `ERROR_LOCK_VIOLATION` as an uncategorized error. The Windows process adapter uses `std::process::Command` with detached flags and null standard streams, but stable Rust still enables generic inheritance of every inheritable parent handle. A caller that captures `start` output can therefore wait forever for pipe EOF while the worker correctly remains alive.

The persisted schema, controller protocol, worker entry point, and Unix launcher already express the intended lifecycle and must remain unchanged. Windows-specific unsafe code must stay inside the platform adapter, and the worker must continue to receive the invoking process's environment so test overrides and per-user path selection work.

## Goals / Non-Goals

**Goals:**

- Classify only the platform-specific error that `fs2` defines as lock contention, without hiding unrelated lock I/O failures.
- Prevent a Windows worker from inheriting unrelated handles while preserving detached, no-console execution and the existing `ProcessLauncher` boundary.
- Keep process creation direct and injection-safe, and close all Win32 handles retained by the launching command.
- Make the lifecycle regression suite fail within a bounded interval if command completion or captured-stream closure regresses.

**Non-Goals:**

- Replacing advisory file locks, changing the persisted state model, or using PID metadata as liveness truth.
- Installing a Windows service, adding login autostart, or changing Unix detachment.
- Changing CLI syntax, acknowledgement policy, scheduling, NetBird command behavior, or force-killing an unresponsive daemon.

## Decisions

### Use the `fs2` contention sentinel rather than an `ErrorKind` or Win32 literal

The lock adapter will compare a failed non-blocking lock's raw OS error with `fs2::lock_contended_error().raw_os_error()`. `ErrorKind::WouldBlock` may remain as a conservative fallback, but the code will not classify every error with the Windows `Uncategorized` kind as contention.

This keeps the operating-system mapping owned by the same crate that performs the lock. Hard-coding error 33 would duplicate `fs2` internals, while comparing only `ErrorKind` is already proven insufficient on Windows and could hide unrelated errors if broadened.

### Use a narrow Win32 process-creation path with handle inheritance disabled

The Unix implementation will continue using `std::process::Command` and `setsid`. The Windows adapter will call `CreateProcessW` through the existing `windows-sys` dependency with:

- the absolute current executable supplied separately as the application name;
- a writable, fixed command line that supplies a neutral `argv[0]` and the internal `__worker` argument without interpolating user input;
- `bInheritHandles` set to false;
- the existing detached, no-window, and new-process-group flags;
- inherited environment and current directory through null optional pointers;
- a zero-initialized `STARTUPINFOW` with its size set and no standard handles supplied.

On success, the adapter will capture the process identifier, immediately close both handles returned in `PROCESS_INFORMATION`, and preserve only the PID in the existing return value. On failure, it will convert `GetLastError` through `std::io::Error::last_os_error` and the existing launch error type.

The stable Rust `CommandExt` API does not currently expose non-inheritance without a nightly feature. Clearing inheritance flags on selected parent handles was rejected because it mutates shared process state, can race with other spawns, and cannot prove that every unrelated inheritable handle was covered. Changing only the tests was rejected because captured CLI output is a real scripting contract, not merely a harness artifact.

### Keep lifecycle tests end-to-end and add bounded failure handling

Existing state/controller tests will continue to exercise a second lock attempt while ownership is held, which becomes a cross-platform regression for the contention classifier. Windows lifecycle coverage will explicitly capture `start` output, observe ready state, and verify that command output reaches EOF while the worker remains live.

Every external test command involved in daemon lifecycle will have a bounded completion path. If the bound expires, the harness will report the command and captured state, request graceful shutdown where possible, and ensure the test process does not wait indefinitely. The normal test continues to verify start, status, concurrent reconfiguration, invalid-update preservation, and stop through the real executable and fake NetBird fixture.

## Risks / Trade-offs

- [The Win32 launcher introduces a small unsafe FFI boundary] → Keep it in `platform.rs`, pass only owned NUL-terminated buffers and null optional pointers, initialize structure sizes explicitly, and close both returned handles on every successful spawn.
- [A non-inheriting worker cannot write early failures to the invoking console] → This matches the current null-stdio contract; retain structured per-user file logging and readiness timeout diagnostics from `start`.
- [Over-broad lock classification could report a failed lock as a live daemon] → Compare against the `fs2` platform sentinel by raw OS code and retain all other errors as `HawkError::Io`.
- [Timing-sensitive process tests can still be slow on loaded CI hosts] → Use bounded polling with margins above normal readiness and shutdown intervals, and include state/log context in timeout failures.

## Migration Plan

1. Update lock classification and prove the existing ownership tests pass on Windows without changing Unix results.
2. Add the minimum `windows-sys` features needed for `CreateProcessW`, structures, and handle cleanup; replace only the Windows launcher internals.
3. Add bounded captured-output lifecycle regression coverage and run formatting, clippy with warnings denied, all targets, and a release build.
4. Let the existing OS matrix validate Windows, Linux, and macOS before release.

No data migration or staged rollout is required. Rollback is a code-only revert because persisted records and public commands do not change.
