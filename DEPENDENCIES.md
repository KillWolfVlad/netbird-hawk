# Dependency and external-command review

Reviewed 2026-08-05. Direct dependencies were selected from their upstream
projects and resolved from crates.io with only the features shown in
`Cargo.toml`. `cargo metadata` confirms their repository and SPDX metadata;
`cargo deny` enforces the allowed license set and RustSec advisories in CI.

The curated [Awesome Rust](https://github.com/rust-unofficial/awesome-rust)
list independently includes the core choices for
[CLI parsing](https://github.com/rust-unofficial/awesome-rust#command-line),
[async runtime](https://github.com/rust-unofficial/awesome-rust#asynchronous),
[date/time](https://github.com/rust-unofficial/awesome-rust#date-and-time),
[temporary files](https://github.com/rust-unofficial/awesome-rust#filesystem), and
[structured logging](https://github.com/rust-unofficial/awesome-rust#logging).
Upstream release activity and the current crates.io resolution were checked at
review time; the automated advisory/source checks guard the lockfile afterward.

| Concern | Crates | Upstream | License |
| --- | --- | --- | --- |
| CLI | `clap` | [clap-rs/clap](https://github.com/clap-rs/clap) | MIT OR Apache-2.0 |
| Async runtime/process/time | `tokio`, `async-trait` | [tokio-rs/tokio](https://github.com/tokio-rs/tokio), [dtolnay/async-trait](https://github.com/dtolnay/async-trait) | MIT; MIT OR Apache-2.0 |
| Local calendar | `chrono` | [chronotope/chrono](https://github.com/chronotope/chrono) | MIT OR Apache-2.0 |
| Serialization | `serde`, `serde_json` | [serde-rs/serde](https://github.com/serde-rs/serde), [serde-rs/json](https://github.com/serde-rs/json) | MIT OR Apache-2.0 |
| Atomic persistence | `tempfile` | [Stebalien/tempfile](https://github.com/Stebalien/tempfile) | MIT OR Apache-2.0 |
| Per-user paths | `directories` | [dirs-dev/directories-rs](https://github.com/dirs-dev/directories-rs) | MIT OR Apache-2.0 |
| File locking | `fs2` | [danburkert/fs2-rs](https://github.com/danburkert/fs2-rs) | MIT OR Apache-2.0 |
| Typed diagnostics | `thiserror` | [dtolnay/thiserror](https://github.com/dtolnay/thiserror) | MIT OR Apache-2.0 |
| Executable discovery | `which` | [harryfei/which-rs](https://github.com/harryfei/which-rs) | MIT |
| Generation IDs | `uuid` | [uuid-rs/uuid](https://github.com/uuid-rs/uuid) | Apache-2.0 OR MIT |
| Structured rolling logs | `tracing`, `tracing-subscriber`, `tracing-appender` | [tokio-rs/tracing](https://github.com/tokio-rs/tracing) | MIT |
| Unix detachment | `nix` (`process` only) | [nix-rust/nix](https://github.com/nix-rust/nix) | MIT |
| Windows detachment | `windows-sys` (`Win32_System_Threading` only) | [microsoft/windows-rs](https://github.com/microsoft/windows-rs) | MIT OR Apache-2.0 |

Development-only `assert_cmd`, `predicates`, and `chrono-tz` exercise CLI
process behavior and deterministic DST fixtures. Their upstream metadata is
also covered by the same license/advisory policy.

## NetBird CLI contract

The supported [NetBird CLI documentation](https://docs.netbird.io/get-started/cli)
documents `netbird status --json`, with `netbird status` as the human-readable
form. The current official status model exposes the active value as JSON
`profileName`. netbird-hawk therefore tries this shell-free invocation first:

```console
netbird status --json
```

For compatible older clients, invalid structured output, or a structured object
without `profileName`, it executes `netbird status` and accepts exactly one
non-empty `Profile:` field independent of line order. Selection uses the
documented [profile command](https://docs.netbird.io/client/profiles):

```console
netbird profile select <target>
```

Both invocations use the absolute executable discovered during preflight, direct
argument arrays (never a shell), null stdin, captured output, and a fixed timeout.
Only allowlisted parsed fields and sanitized failure categories leave the
in-memory command adapter.
