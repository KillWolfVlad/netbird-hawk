# netbird-hawk

<p align="center">
  <img src="./brand/rufus-hawk-wimbledon.jpg" width="200"/>
  <br>
  This project is named after <a href="https://en.wikipedia.org/wiki/Rufus_the_Hawk">Rufus the Hawk</a>,
  the bird of prey employed by Wimbledon to keep pigeons away from its tennis courts.
</p>

`netbird-hawk` is a per-user background daemon that rotates through an ordered
list of NetBird profiles at a predictable local time. It makes a roughly daily
SSO renewal window intentional instead of letting it drift into the workday.

The daemon supports macOS, Linux, and Windows. It does not install a system
service, require elevated privileges, manage NetBird credentials, or start
automatically at boot/login.

## Prerequisites and installation

- A current Rust toolchain (edition 2024 support is required to build from source).
- A supported NetBird client with the `netbird` executable on `PATH`.
- Every configured NetBird profile created and authenticated in advance. The
  daemon never opens an SSO flow; interactive authentication remains a deliberate
  user action.

Build a release binary:

```console
cargo build --release
```

Install `target/release/netbird-hawk` (or `netbird-hawk.exe` on Windows) in a
directory on your `PATH`.

## Usage

Start a daily 08:24 rotation through two profiles:

```console
netbird-hawk start --profile default -p default2 --time 8:24
```

`--profile`/`-p` is repeatable, and order matters. At each due occurrence the
daemon reads the active profile from `netbird status`, selects the next profile
in the supplied order, and wraps from the last profile to the first. If the
active profile was changed manually, that current value determines the next
target. If it is outside the list, the first configured profile is selected.
Profile handles must be non-empty and unique.

The time is the computer's local wall time in `H:MM` or `HH:MM` 24-hour format.
A newly applied configuration never runs immediately when today's time has
already passed; it starts the next day. After sleep or downtime, an existing
generation catches up only the latest due date. Timezone and clock changes are
re-evaluated frequently. A daylight-saving gap runs at the first valid instant
after the gap; a repeated time uses its first occurrence.

Lifecycle commands:

```console
netbird-hawk status
netbird-hawk stop
netbird-hawk --help
netbird-hawk help start
```

Running `start` again first validates and preflights the whole replacement,
then atomically applies it to the existing daemon. `stop` is idempotent and
waits for graceful shutdown. `status` reports liveness from the daemon lock—not
from stale PID metadata—and includes the ordered profiles, local time, active
and next profiles, next occurrence, and sanitized last result when available.

## Scheduling and recovery guarantees

Each configuration has an immutable generation ID. Each occurrence is identified
by generation and local date. Before `netbird profile select <target>` runs,
netbird-hawk durably writes the original and intended profiles. After a crash it
rechecks NetBird:

- the intended target already active completes without selecting again;
- the original still active permits a bounded retry;
- a third profile is treated as a manual override and is not overwritten.

Status and selection failures use delayed, bounded retries. Retries stop after
success, manual supersession, exhaustion, a newer local-date occurrence,
configuration replacement, or daemon stop. No command is constructed through a
shell.

## Per-user files

Locations follow OS conventions through the Rust `directories` crate. XDG
variables replace the shown Linux defaults when set.

| OS | Configuration | Runtime state and locks | Logs |
| --- | --- | --- | --- |
| Linux | `~/.config/netbird-hawk/config.json` | `~/.local/state/netbird-hawk/` | `~/.local/share/netbird-hawk/logs/` |
| macOS | `~/Library/Application Support/netbird-hawk/config.json` | `~/Library/Application Support/netbird-hawk/` | `~/Library/Application Support/netbird-hawk/logs/` |
| Windows | `%APPDATA%\netbird-hawk\config\config.json` | `%LOCALAPPDATA%\netbird-hawk\data\` | `%LOCALAPPDATA%\netbird-hawk\data\logs\` |

Unix directories are forced to mode `0700` and files to `0600`. Windows files
inherit the current user's profile ACLs; netbird-hawk never broadens them.
Structured logs retain seven daily files.

Only configuration, generation/date identity, intended profile transitions,
PID diagnostics, and allowlisted error categories are persisted. Raw NetBird
stdout/stderr, credentials, tokens, cookies, and authorization codes are never
written to state or normal logs. Profile names themselves are private metadata,
so keep these per-user directories private.

`NETBIRD_HAWK_HOME` may point to an absolute alternate root for isolated testing;
the config, state, and log directories are created beneath it.

## Troubleshooting

**`netbird` was not found:** ensure the NetBird CLI is installed and that its
directory is on the `PATH` visible to the command. Run `netbird status` yourself
to confirm the current user can reach the NetBird daemon.

**Status is malformed or degraded:** current versions prefer the documented
`netbird status --json` `profileName` field and fall back to exactly one non-empty
`Profile:` field from `netbird status`. Upgrade NetBird if neither form is
available. Raw output is intentionally omitted from diagnostics; inspect NetBird
directly in a trusted terminal when needed.

**A profile selection fails:** confirm the handle is unique and already
authenticated with `netbird profile list --show-id` and a deliberate manual
selection. NetBird permits duplicate display names, but netbird-hawk requires
unique handles so rotation is deterministic.

**`stop` times out:** retry `netbird-hawk stop` and inspect the per-user log.
PID data is diagnostic only, so netbird-hawk will never kill a process from that
number. If manual termination is necessary, use the OS process manager and verify
both the executable path and current user before ending the process; logging out
or rebooting is the safest fallback. Do not delete the lock file to manufacture a
stopped state. Once no daemon owns the lock, `status` ignores stale snapshots.

**After reboot/login:** run `netbird-hawk start` again. This release deliberately
does not install launchd, systemd, a Windows service, or any boot/login autostart.

## Development

```console
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo build --release
cargo deny check licenses advisories sources
```

See [DEPENDENCIES.md](DEPENDENCIES.md) for the dependency and NetBird CLI
contract review.

## Maintainers

- [@KillWolfVlad](https://github.com/KillWolfVlad)

## License

This repository is released under version 2.0 of the
[Apache License](https://www.apache.org/licenses/LICENSE-2.0).
