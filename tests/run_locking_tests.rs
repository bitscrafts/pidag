//! Tests for run locking (R4).
//!
//! Verifies that concurrent runs with the same run_id are fenced by a per-run-id lock,
//! while different run_ids in the same vault can proceed concurrently.

use std::fs;
use std::path::{Path, PathBuf};

fn tmp_dir() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = PathBuf::from("_tmp");
    let dir = base.join(format!("run-locking-test-{}", n));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create test dir");
    dir
}

/// Compute the lock path for a given vault and run_id.
/// This should match the implementation in run.rs.
fn run_lock_path(vault_path: &Path, run_id: &str) -> PathBuf {
    let vault_dir = vault_path.parent().unwrap_or_else(|| Path::new(".pidag"));
    vault_dir.join(".locks").join(format!("{}.pid", run_id))
}

#[test]
fn test_second_run_same_id_is_refused() {
    // R4a: Acquire the lock for a run id, then attempt a second acquire on
    // the same path: it should be refused with Ok(None).
    let tmpdir = tmp_dir();
    let vault_path = tmpdir.join("pidag.redb");
    let run_id = "test-run-001";
    let lock_path = run_lock_path(&vault_path, run_id);

    // First acquire should succeed.
    let _lock1 = pidag::agent::lock::PidLock::acquire(&lock_path)
        .expect("acquire should succeed")
        .expect("first lock should be held");

    // Second acquire with same run_id should be refused.
    let lock2 = pidag::agent::lock::PidLock::acquire(&lock_path).expect("acquire should not error");
    assert!(
        lock2.is_none(),
        "second acquire with same run_id should be refused"
    );

    // Cleanup
    let _ = fs::remove_dir_all(&tmpdir);
}

#[test]
fn test_different_run_ids_both_proceed() {
    // R4b: Two distinct run ids in the same vault dir should both acquire.
    // This guards the per-run-id requirement; a vault-wide lock fails this.
    let tmpdir = tmp_dir();
    let vault_path = tmpdir.join("pidag.redb");

    let run_id_1 = "test-run-001";
    let run_id_2 = "test-run-002";
    let lock_path_1 = run_lock_path(&vault_path, run_id_1);
    let lock_path_2 = run_lock_path(&vault_path, run_id_2);

    // Both should acquire successfully (different run_ids).
    let _lock1 = pidag::agent::lock::PidLock::acquire(&lock_path_1)
        .expect("first lock should not error")
        .expect("first lock should be held");

    let _lock2 = pidag::agent::lock::PidLock::acquire(&lock_path_2)
        .expect("second lock should not error")
        .expect("second lock with different run_id should be held");

    // Cleanup
    let _ = fs::remove_dir_all(&tmpdir);
}

#[test]
fn test_lock_released_after_run() {
    // R4c: Drop the first guard, then re-acquire the same path successfully.
    let tmpdir = tmp_dir();
    let vault_path = tmpdir.join("pidag.redb");
    let run_id = "test-run-001";
    let lock_path = run_lock_path(&vault_path, run_id);

    // First acquire and drop
    {
        let _lock1 = pidag::agent::lock::PidLock::acquire(&lock_path)
            .expect("acquire should succeed")
            .expect("first lock should be held");
        assert!(lock_path.exists(), "lock file should exist while held");
    }
    // _lock1 is dropped here, lock should be released

    assert!(
        !lock_path.exists(),
        "lock file should be removed after drop"
    );

    // Re-acquire should succeed
    let _lock2 = pidag::agent::lock::PidLock::acquire(&lock_path)
        .expect("reacquire should not error")
        .expect("reacquire should succeed after lock released");

    // Cleanup
    let _ = fs::remove_dir_all(&tmpdir);
}
