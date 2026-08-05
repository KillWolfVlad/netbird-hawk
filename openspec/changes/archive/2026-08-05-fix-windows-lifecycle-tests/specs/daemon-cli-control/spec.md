## ADDED Requirements

### Requirement: Daemon ownership and detachment are platform-safe
The application SHALL interpret the supported operating system's non-blocking lock-contention result as evidence that another process owns the daemon instance, while preserving other lock failures as state I/O errors. A background worker SHALL NOT inherit unrelated process handles from the invoking command, and completion of a successful public command SHALL NOT depend on the lifetime of the detached worker.

#### Scenario: Windows lifetime lock is already owned
- **WHEN** a Windows daemon holds the current user's lifetime lock and another lifecycle command probes that lock
- **THEN** the command treats the daemon as live and continues with the applicable status, reconfiguration, or shutdown behavior instead of reporting a lock I/O failure

#### Scenario: An unexpected lock operation fails
- **WHEN** a lifetime-lock probe fails for a reason other than the operating system's documented contention result
- **THEN** the command exits unsuccessfully with a state I/O diagnostic and does not claim that the daemon is running or stopped

#### Scenario: Windows start output is captured
- **WHEN** a caller captures the Windows `start` command's standard output and error while the new worker reaches ready state
- **THEN** `start` prints its result, closes the caller-visible streams, and exits successfully while the detached worker remains running

#### Scenario: Invoking terminal closes after Windows start
- **WHEN** the Windows command that launched a ready background worker exits or its terminal closes
- **THEN** the worker retains no unrelated handles from that command and continues under its own daemon lifetime ownership
