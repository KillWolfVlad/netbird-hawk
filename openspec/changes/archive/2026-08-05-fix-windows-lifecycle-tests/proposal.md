## Why

Windows lifecycle checks fail because an occupied `fs2` lock is reported as Win32 `ERROR_LOCK_VIOLATION` instead of `ErrorKind::WouldBlock`, and a detached worker can inherit the invoking command's capture pipes. These platform semantics make healthy daemon ownership look like an I/O failure and can prevent `start` or its integration test from returning after the worker is ready.

## What Changes

- Recognize lock contention through the cross-platform `fs2` contention contract so Windows liveness checks distinguish an owned daemon lock from an unexpected state I/O failure.
- Launch the Windows worker without inheriting unrelated process handles while retaining detached, no-console, per-user behavior.
- Add bounded Windows-focused regression coverage for lock ownership and captured-output process launch so failures terminate diagnostically instead of hanging CI.
- Re-run the complete lifecycle suite on Windows, Linux, and macOS to preserve the existing cross-platform contract.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `daemon-cli-control`: Clarify that daemon liveness and terminal-independent launch must honor Windows lock-error and handle-inheritance semantics as well as Unix behavior.

## Impact

- Affected implementation: `src/state.rs` lock classification and the Windows adapter in `src/platform.rs`.
- Affected tests: state/controller unit tests and `tests/lifecycle.rs` process-lifecycle coverage.
- Windows bindings may need narrowly scoped `windows-sys` features for non-inheriting process creation and handle cleanup.
- No CLI syntax, persisted schema, scheduling behavior, or public Rust API is intentionally changed.
