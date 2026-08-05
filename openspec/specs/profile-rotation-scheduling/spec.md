# profile-rotation-scheduling Specification

## Purpose

Defines reliable local-time scheduling and deterministic ordered profile selection while remaining correct after manual changes, sleep, restart, and external-command failures.

## Requirements

### Requirement: Active profile is discovered from NetBird status
The application SHALL execute `netbird status` without a command shell at daemon startup and before each scheduled rotation. It SHALL extract the value from a single `Profile:` field regardless of the ordering of other status fields, and SHALL treat a missing, empty, or ambiguous profile field or an unsuccessful command as an unavailable active profile.

#### Scenario: NetBird returns the documented status shape
- **WHEN** `netbird status` succeeds and contains exactly one non-empty line such as `Profile: default`
- **THEN** the application records `default` as the active profile without depending on the other status lines

#### Scenario: NetBird status cannot identify one profile
- **WHEN** the command fails, times out, or returns no unique non-empty `Profile:` value
- **THEN** the application records a sanitized error, does not invoke `netbird profile select` for that attempt, and reports degraded state

### Requirement: Rotation follows the configured circular order
For each due occurrence, the application SHALL choose the profile immediately after the active profile in the configured ordered list and SHALL wrap from the last profile to the first. If the active profile is not in the configured list, it SHALL choose the first configured profile. A one-profile list SHALL select that profile.

#### Scenario: Active profile is in the middle of the list
- **WHEN** the configured profiles are `alpha`, `beta`, `gamma` and the active profile is `beta`
- **THEN** the target is `gamma`

#### Scenario: Active profile is last in the list
- **WHEN** the configured profiles are `alpha`, `beta`, `gamma` and the active profile is `gamma`
- **THEN** the target wraps to `alpha`

#### Scenario: Active profile is outside the list
- **WHEN** the configured profiles are `alpha`, `beta` and the active profile is `manual`
- **THEN** the target is `alpha`

#### Scenario: Only one profile is configured
- **WHEN** the configured profile is `alpha`
- **THEN** the target is `alpha`

### Requirement: Manual profile changes influence the next rotation
The application SHALL rediscover the active profile at the time of every scheduled occurrence rather than relying on the previously selected profile. A manual selection made before that occurrence SHALL therefore determine the next configured profile.

#### Scenario: User switches to another configured profile
- **WHEN** the last daemon-selected profile was `alpha`, the user manually selects `beta`, and the configured order is `alpha`, `beta`, `gamma`
- **THEN** the next scheduled target is `gamma`

#### Scenario: User switches to an unconfigured profile
- **WHEN** the user manually selects `manual` before the occurrence and that profile is not in the configured list
- **THEN** the next scheduled target is the first configured profile

### Requirement: Scheduling uses the computer's current local wall clock
The daemon SHALL interpret the configured time in the operating system's current local timezone and create one scheduled occurrence per local calendar date. It SHALL recalculate the next occurrence after startup, configuration replacement, wake, and clock or timezone change, and SHALL execute no earlier than the applicable local time.

#### Scenario: The configured local time arrives
- **WHEN** the daemon is running continuously and the local wall clock reaches the configured time on a date not yet handled
- **THEN** the daemon begins one rotation occurrence for that local date

#### Scenario: Configuration is activated after today's time
- **WHEN** a new configuration generation is applied after its configured time on the current local date
- **THEN** its first normal occurrence is scheduled for the next local date rather than running immediately

#### Scenario: The timezone or clock changes before the occurrence
- **WHEN** the computer's local timezone or wall clock changes while the daemon is waiting
- **THEN** the daemon recomputes the occurrence from the new local calendar and still handles at most one occurrence for a local date

#### Scenario: The local time is skipped by a daylight-saving transition
- **WHEN** the configured wall time does not exist on a local date
- **THEN** that date's occurrence becomes due at the first valid local instant after the gap

#### Scenario: The local time occurs twice during a daylight-saving transition
- **WHEN** the configured wall time is ambiguous on a local date
- **THEN** the first occurrence is used and no second rotation is performed for that date

### Requirement: Missed occurrences are caught up once
The daemon SHALL durably identify handled occurrences by configuration generation and local calendar date. If an occurrence becomes due while the process is suspended, sleeping, or interrupted, it SHALL begin or reconcile that occurrence promptly after resuming, but SHALL NOT replay every missed date or perform more than one successful selection for the same occurrence.

#### Scenario: Computer wakes after today's configured time
- **WHEN** the daemon was active before sleep, no attempt was recorded for today, and the computer wakes after today's configured time
- **THEN** the daemon begins one catch-up occurrence for today

#### Scenario: Several dates elapsed while the daemon was not running
- **WHEN** the daemon restarts with its existing generation after multiple scheduled dates were missed
- **THEN** it reconciles only the latest due local-date occurrence and does not perform a burst of historical rotations

#### Scenario: A completed occurrence is observed again
- **WHEN** restart or a backward clock change causes the daemon to evaluate a generation and local date already marked successful
- **THEN** no additional profile selection is performed for that occurrence

### Requirement: Profile selection is invoked safely and reconciled
For a due occurrence, the daemon SHALL persist its intended target before executing `netbird profile select <target>` directly without a command shell. The command SHALL have a bounded timeout. On restart or retry, the daemon SHALL inspect the active profile: an active profile equal to the intended target completes the occurrence without another selection, the unchanged original profile permits a retry of the same target, and a different profile supersedes retries as a manual change.

#### Scenario: Profile selection succeeds
- **WHEN** the target is `beta` and `netbird profile select beta` exits successfully within the timeout
- **THEN** the occurrence is marked successful and the published active profile becomes `beta`

#### Scenario: Process stops after selection but before success is recorded
- **WHEN** the saved occurrence target is `beta` and startup discovery reports `beta`
- **THEN** the daemon marks that occurrence successful without invoking profile selection again

#### Scenario: A retry sees the original profile
- **WHEN** an attempt targeting `beta` fails and status still reports the original profile `alpha`
- **THEN** the daemon may retry the same target under the bounded retry policy

#### Scenario: A retry sees a manual change
- **WHEN** an attempt targeting `beta` is pending but status reports a profile that is neither the original profile nor `beta`
- **THEN** the daemon records that the occurrence was superseded and does not overwrite the manual choice during that occurrence

### Requirement: Failures use bounded recovery without rotation loops
Transient status and selection failures SHALL be retried with a bounded delayed policy for the current occurrence. Retries SHALL stop when the occurrence succeeds, is superseded by manual action, reaches its retry limit, the configuration changes, the daemon stops, or the next scheduled occurrence becomes due. Failures SHALL remain visible in daemon status and logs without causing an immediate unbounded loop.

#### Scenario: A transient command failure recovers
- **WHEN** an external command initially fails and a later bounded retry succeeds for the same occurrence
- **THEN** the occurrence is recorded as successful exactly once

#### Scenario: The retry limit is reached
- **WHEN** all bounded retries for an occurrence fail
- **THEN** the daemon remains running in degraded state, exposes a sanitized last error, and waits until the next scheduled occurrence or a new configuration

#### Scenario: Configuration changes during retries
- **WHEN** a new configuration generation is applied while an old occurrence has a retry pending
- **THEN** the old retry is cancelled and cannot select a profile after the new generation is acknowledged

