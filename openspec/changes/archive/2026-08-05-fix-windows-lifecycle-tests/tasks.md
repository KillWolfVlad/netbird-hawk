## 1. Cross-Platform Lock Contention

- [x] 1.1 Update lifetime-lock probing to recognize the raw OS error exposed by `fs2::lock_contended_error`, retain `WouldBlock` as an appropriate fallback, and propagate every other lock failure as state I/O.
- [x] 1.2 Extend state/controller tests to prove an owned lifetime lock reports a live daemon and that the resulting `status` and bounded `stop` paths behave identically on Windows and Unix.

## 2. Windows Detached Process Launch

- [x] 2.1 Enable only the additional `windows-sys` features required for `CreateProcessW`, its security parameter types, process information, and handle cleanup.
- [x] 2.2 Replace the Windows `std::process::Command` spawn internals with a narrowly scoped `CreateProcessW` adapter that uses the absolute executable, fixed `__worker` arguments, inherited environment, detached flags, and disabled generic handle inheritance.
- [x] 2.3 Validate the Win32 FFI boundary, error conversion, PID result, and closure of both returned process handles with focused Windows tests or assertions while leaving the `ProcessLauncher` interface and Unix adapter unchanged.

## 3. Lifecycle Regression Coverage

- [x] 3.1 Add bounded completion and diagnostic cleanup to the integration harness so a command or captured stream cannot hang the test job indefinitely.
- [x] 3.2 Add a Windows regression that captures `start` output, observes EOF after readiness, confirms the worker remains live, and then stops it gracefully.
- [x] 3.3 Re-run the existing detached start, status, concurrent reconfiguration, invalid-update preservation, repeated stop, fake command failure, and timeout scenarios with no lingering worker processes.

## 4. Cross-Platform Verification

- [x] 4.1 Run `cargo fmt --all -- --check` and `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] 4.2 Run `cargo test --all-targets` and `cargo build --release` on Windows, confirming lifecycle tests finish within their bounds and leave no worker processes.
- [x] 4.3 Confirm the CI matrix passes the same formatting, lint, test, and release-build checks on Linux and macOS without changing their detachment or lock behavior.
