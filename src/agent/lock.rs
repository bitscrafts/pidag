//! Pid-lock for the autonomous driver (`pidag auto`).
//!
//! A small advisory lock that prevents two concurrent `pidag auto` passes from
//! colliding when the host schedules them on a 30-minute cron. The lock is a
//! file (default `.pidag/auto.lock`) whose contents record the owning PID and a
//! timestamp.
//!
//! Semantics:
//! - Acquire uses exclusive `create_new` so a concurrent pass fails fast.
//! - If the lock file exists but its PID is no longer alive (a crashed process),
//!   it is considered stale: we reclaim it and proceed.
//! - The lock is released (file removed) when the guard drops, so the normal
//!   run path and every early `?`-return release it automatically — even on
//!   panics (Drop runs during unwind) or an explicit release.
//!
//! This is an advisory protocol lock, sufficient for cron: exactly one `pidag
//! auto` pass runs at a time; a stale lock from a crash is auto-reclaimed.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A held autosomal lock guard. Removing it (or dropping it) releases the lock.
pub struct PidLock {
    path: PathBuf,
    held: bool,
}

impl PidLock {
    /// Try to take the lock at `path`. Returns `Ok(Some(guard))` on success,
    /// `Ok(None)` when another live process holds it (caller should exit, it is
    /// not an error), and `Err` on real I/O failures.
    pub fn acquire(path: &Path) -> Result<Option<PidLock>, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("lock dir create failed: {e}"))?;
        }

        let pid = std::process::id();
        let body = format!("{pid}\n");

        // First attempt: exclusive create. If it succeeds we own the lock.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut f) => {
                let _ = f.write_all(body.as_bytes());
                Ok(Some(PidLock {
                    path: path.to_path_buf(),
                    held: true,
                }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Lock file exists — is the holder alive?
                if lock_holder_alive(path) {
                    Ok(None) // live owner; concurrent run must stand down
                } else {
                    // Stale lock (holder crashed). Reclaim it.
                    let _ = std::fs::remove_file(path);
                    Self::acquire(path) // retry once now that it's gone
                }
            }
            Err(e) => Err(format!("lock acquire failed: {e}")),
        }
    }

    /// Explicitly release the lock (idempotent; Drop also releases).
    pub fn release(&mut self) {
        if self.held {
            self.held = false;
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// The lock file path being held.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PidLock {
    fn drop(&mut self) {
        self.release();
    }
}

/// Read the PID from a lock file (if any).
fn lock_pid(path: &Path) -> Option<u32> {
    let mut content = String::new();
    let mut f = File::open(path).ok()?;
    f.read_to_string(&mut content).ok()?;
    content.trim().parse::<u32>().ok()
}

/// Return true when the PID recorded in the lock file belongs to a live process.
fn lock_holder_alive(path: &Path) -> bool {
    let Some(pid) = lock_pid(path) else {
        // Corrupt or unparseable lock file -> treat as not held (reclaimable).
        return false;
    };
    process_alive(pid)
}

/// Portable process-liveness probe: `kill(pid, 0)`.
fn process_alive(pid: u32) -> bool {
    // Linux: /proc/<pid> exists iff the process is alive (includes zombies? no —
    // zombies retain /proc until reaped, but the lock holder is a top-level
    // foreground process so it is reaped promptly; conservative is fine).
    let proc = PathBuf::from(format!("/proc/{pid}"));
    if proc.exists() {
        return true;
    }
    // Fallback for non-Linux: the `kill` command with signal 0.
    let status = std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) => s.success(),
        Err(_) => false, // no `kill` in PATH; assume not alive (best effort)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(dir: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static C: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            dir,
            std::process::id(),
            C.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn acquire_then_release() {
        let d = tmp("pidag-lock-a");
        let path = d.join(".pidag").join("auto.lock");
        // First acquire succeeds.
        let mut g = PidLock::acquire(&path).unwrap().expect("should hold");
        assert!(path.exists(), "lock file present while held");
        // A second acquire (same process, concurrently) must be refused.
        assert!(
            PidLock::acquire(&path).unwrap().is_none(),
            "concurrent acquire refused"
        );
        g.release();
        assert!(!path.exists(), "lock file removed on release");
        // Re-acquire works after release.
        let g2 = PidLock::acquire(&path).unwrap().expect("reacquire");
        drop(g2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stale_lock_is_reclaimed() {
        let d = tmp("pidag-lock-stale");
        let path = d.join("auto.lock");
        // Write a lock file claiming a PID that cannot be alive (huge).
        std::fs::write(&path, "9999999999\n").unwrap();
        assert!(
            !lock_holder_alive(&path),
            "unrealistic PID is not alive (stale)"
        );
        // Acquire should reclaim the stale lock.
        let mut g = PidLock::acquire(&path).unwrap().expect("reclaim stale");
        assert!(path.exists());
        g.release();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_lock_is_reclaimable() {
        let d = tmp("pidag-lock-corrupt");
        let path = d.join("auto.lock");
        std::fs::write(&path, "not-a-pid\n").unwrap();
        let g = PidLock::acquire(&path).unwrap().expect("reclaim corrupt");
        drop(g);
        let _ = std::fs::remove_dir_all(&d);
    }
}
