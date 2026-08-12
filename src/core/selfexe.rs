//! Resolving pidag's own executable path.
//!
//! Several subcommands re-invoke pidag as a subprocess: `sdd --run` spawns
//! `pidag run`, and the queue daemon, queue executor and autonomous driver
//! each spawn further subcommands. Every one of them used
//! `Command::new("pidag")`, which resolves through `PATH` — so a freshly
//! built binary handed the actual work to whichever copy happened to be
//! installed.
//!
//! That is the structural cause of the "verified against a stale binary"
//! failures recorded in `CLAUDE.md` rule 4: a developer can rebuild, run
//! `pidag sdd --run`, watch it pass, and have exercised the old code. It bit
//! twice on 2026-08-12 alone — once hiding a template change, once hiding
//! spec-29 entirely.
//!
//! Resolving via `current_exe()` makes a binary always delegate to itself.

use std::path::PathBuf;

/// Path to the running pidag executable, for re-invoking a subcommand.
///
/// Falls back to the bare name `pidag` (PATH lookup) only when
/// `current_exe()` fails, which on Linux means `/proc/self/exe` was
/// unreadable — rare, and the old behaviour is the best available guess.
pub fn self_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("pidag"))
}
