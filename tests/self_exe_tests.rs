//! Audit S-7: pidag must re-invoke ITSELF, not whatever `pidag` is on PATH.
//!
//! `pidag sdd --run` spawns `pidag run` as a subprocess; the queue daemon,
//! queue executor and autonomous driver each spawn subcommands too. When
//! those resolved the bare name through PATH, a freshly built binary handed
//! the real work to whichever copy was installed — so a rebuild could be
//! "verified" while the old code ran. It hid a template change and then hid
//! spec-29 entirely on 2026-08-12.

use std::path::Path;

#[test]
fn test_self_exe_returns_an_absolute_existing_path() {
    let p = pidag::self_exe();
    assert!(
        p.is_absolute(),
        "self_exe() must be absolute so the child does not depend on PATH, got {:?}",
        p
    );
    assert!(
        p.exists(),
        "self_exe() must point at a real file, got {:?}",
        p
    );
}

#[test]
fn test_self_exe_is_not_the_bare_name() {
    let p = pidag::self_exe();
    assert_ne!(
        p,
        Path::new("pidag"),
        "self_exe() fell back to the bare name; the child would resolve via PATH \
         and could execute a different build"
    );
}

/// The regression guard. Any `Command::new("pidag")` reintroduces the trap,
/// so scan the sources rather than trusting review to catch it.
#[test]
fn test_no_bare_name_self_invocation_in_sources() {
    let mut offenders = Vec::new();
    for entry in walk("src") {
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        for (i, line) in text.lines().enumerate() {
            // Skip doc comments: selfexe.rs documents the old pattern.
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains(r#"Command::new("pidag")"#) {
                offenders.push(format!("{}:{}", entry.display(), i + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these sites resolve pidag via PATH and can execute a stale binary; \
         use crate::core::selfexe::self_exe() instead:\n  {}",
        offenders.join("\n  ")
    );
}

fn walk(dir: &str) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(dir)];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out
}
